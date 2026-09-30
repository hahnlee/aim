package dev.aim.server;

import android.content.BroadcastReceiver;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.app.AppGlobals;
import android.content.pm.ApplicationInfo;
import android.content.pm.IPackageManager;
import android.content.pm.PackageManager;
import android.database.IContentObserver;
import android.location.ILocationManager;
import android.location.LocationManager;
import android.location.LocationManagerInternal;
import android.location.LocationTime;
import android.location.util.identity.CallerIdentity;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.PackageTagsList;
import android.os.PowerManagerInternal;
import android.os.PowerSaveState;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.util.Slog;

import com.android.server.FgThread;
import com.android.server.LocalServices;
import com.android.server.SystemConfig;
import com.android.server.pm.UserManagerInternal;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import com.android.server.servicewatcher.CurrentUserServiceSupplier;
import com.android.server.servicewatcher.CurrentUserServiceSupplier.BoundServiceInfo;
import com.android.server.servicewatcher.ServiceWatcher;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;

/**
 * The system_server side of the native location service (ADR 0013,
 * docs/system-services.md): what the original LocationManagerService does
 * inside system_server, for the service host. It binds the providers that
 * apps serve with the original's ServiceWatcher and hands the host their
 * binders, tells it of users, power save and the screen, observes settings
 * on its behalf, and publishes the LocationManagerInternal that
 * system_server's own code (AppOpsPolicy) consults.
 */
final class LocationBridge extends ILocationBridge.Stub {
    private static final String TAG = "AimLocationBridge";

    /** The actions and config of the bound providers (LocationManagerService). */
    private static final String ACTION_NETWORK_PROVIDER =
            "com.android.location.service.v3.NetworkLocationProvider";
    private static final String ACTION_FUSED_PROVIDER =
            "com.android.location.service.FusedLocationProvider";
    private static final String ACTION_GEOCODE_PROVIDER =
            "com.android.location.service.GeocodeProvider";
    private static final String ACTION_POPULATION_DENSITY_PROVIDER =
            "com.android.location.service.PopulationDensityProvider";
    /** ProxyLocationProvider's metadata key of a provider's extra tags. */
    private static final String EXTRA_LOCATION_TAGS = "android:location_allow_listed_tags";
    /** PowerManager.ServiceType.LOCATION and PowerManager.LOCATION_MODE_NO_CHANGE. */
    private static final int SERVICE_TYPE_LOCATION = 1;
    private static final int LOCATION_MODE_NO_CHANGE = 0;

    private final Context mContext;
    private final Object mLock = new Object();
    /** Whether the native service stands in for LocationManagerService. */
    private boolean mActive;
    private ILocationHost mHost;
    /** What the host has been told of the bound providers, told again to a new host. */
    private String[] mResolved;
    private final Map<String, Object[]> mBound = new HashMap<>();
    private LocationManagerInternal.LocationPackageTagsListener mTagsListener;
    private final Map<Integer, PackageTagsList> mTags = new HashMap<>();
    private boolean mReceiversRegistered;
    private final Map<String, ServiceWatcher> mWatchers = new HashMap<>();

    LocationBridge(Context context) {
        mContext = context;
    }

    /**
     * At SystemService.onStart, where the native service stands in for
     * LocationManagerService (which publishes LocationManagerInternal
     * itself otherwise): LocationManagerInternal for system_server's code,
     * and the location provider packages of the default grants, as
     * LocationManagerService's constructor sets them. PackageManagerService
     * made the grants before this service started, so on a first boot or
     * an upgrade they are made again with the packages known.
     */
    void publish() {
        if (LocalServices.getService(LocationManagerInternal.class) != null) {
            return;
        }
        mActive = true;
        LocalServices.addService(LocationManagerInternal.class, new Internal());
        LegacyPermissionManagerInternal permissions =
                LocalServices.getService(LegacyPermissionManagerInternal.class);
        String[] packages = stringArray("config_locationProviderPackageNames");
        String[] extraPackages = stringArray("config_locationExtraPackageNames");
        permissions.setLocationPackagesProvider(userId -> packages);
        permissions.setLocationExtraPackagesProvider(userId -> extraPackages);
        IPackageManager pm = AppGlobals.getPackageManager();
        try {
            if (pm.isFirstBoot() || pm.isDeviceUpgrading()) {
                for (int userId : LocalServices.getService(UserManagerInternal.class).getUserIds()) {
                    permissions.grantDefaultPermissions(userId);
                }
            }
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
    }

    /**
     * IBridge.getLocationBridge: the host of the native location service;
     * none while the original runs.
     */
    ILocationBridge attach(ILocationHost host) {
        if (!mActive) {
            return null;
        }
        String[] resolved;
        ArrayList<Object[]> bound;
        synchronized (mLock) {
            mHost = host;
            resolved = mResolved;
            bound = new ArrayList<>(mBound.values());
            if (!mReceiversRegistered) {
                mReceiversRegistered = true;
                registerReceivers();
            }
        }
        try {
            if (resolved != null) {
                host.onProvidersResolved(resolved);
                for (Object[] b : bound) {
                    host.onProviderBound((String) b[0], (IBinder) b[1], (String) b[2],
                            (String) b[3]);
                }
            }
        } catch (RemoteException e) {
            Slog.w(TAG, "the location host died", e);
        }
        return this;
    }

    /**
     * At PHASE_THIRD_PARTY_APPS_CAN_START, as onSystemThirdPartyAppsCanStart
     * binds them: the providers apps serve.
     */
    void onThirdPartyAppsCanStart() {
        if (!mActive) {
            return;
        }
        ArrayList<String> resolved = new ArrayList<>();
        ArrayList<ServiceWatcher> watchers = new ArrayList<>();
        ServiceWatcher network = ServiceWatcher.create(mContext, "network",
                supplier(ACTION_NETWORK_PROVIDER, "config_enableNetworkLocationOverlay",
                        "config_networkLocationProviderPackageName"),
                new Listener("network"));
        ServiceWatcher fused = ServiceWatcher.create(mContext, "fused",
                bool("config_fusedLocationOverlayUnstableFallback"),
                supplier(ACTION_FUSED_PROVIDER, "config_enableFusedLocationOverlay",
                        "config_fusedLocationProviderPackageName"),
                new Listener("fused"));
        ServiceWatcher geocoder = ServiceWatcher.create(mContext, "GeocoderProxy",
                supplier(ACTION_GEOCODE_PROVIDER, "config_enableGeocoderOverlay",
                        "config_geocoderProviderPackageName"),
                new Listener("geocoder"));
        ServiceWatcher density = ServiceWatcher.create(mContext, "PopulationDensityProxy",
                supplier(ACTION_POPULATION_DENSITY_PROVIDER,
                        "config_enablePopulationDensityProviderOverlay",
                        "config_populationDensityProviderPackageName"),
                new Listener("density"));
        String[] names = {"network", "fused", "geocoder", "density"};
        ServiceWatcher[] all = {network, fused, geocoder, density};
        for (int i = 0; i < all.length; i++) {
            if (all[i].checkServiceResolves()) {
                resolved.add(names[i]);
                watchers.add(all[i]);
            }
        }
        ILocationHost host;
        synchronized (mLock) {
            for (int i = 0; i < all.length; i++) {
                if (watchers.contains(all[i])) {
                    mWatchers.put(names[i], all[i]);
                }
            }
            mResolved = resolved.toArray(new String[0]);
            host = mHost;
        }
        if (host != null) {
            try {
                host.onProvidersResolved(mResolved);
            } catch (RemoteException e) {
                Slog.w(TAG, "the location host died", e);
            }
        }
        for (ServiceWatcher w : watchers) {
            w.register();
        }
    }

    void onUserStarting(int userId) {
        host(h -> h.onUserStarting(userId));
    }

    void onUserStopped(int userId) {
        host(h -> h.onUserStopped(userId));
    }

    void onUserSwitching(int fromUserId, int toUserId) {
        host(h -> h.onUserSwitching(fromUserId, toUserId));
    }

    private CurrentUserServiceSupplier supplier(String action, String enableOverlay,
            String nonOverlayPackage) {
        return CurrentUserServiceSupplier.createFromConfig(mContext, action,
                resource(enableOverlay, "bool"), resource(nonOverlayPackage, "string"));
    }

    private int resource(String name, String type) {
        return mContext.getResources().getIdentifier(name, type, "android");
    }

    private boolean bool(String name) {
        return mContext.getResources().getBoolean(resource(name, "bool"));
    }

    private String[] stringArray(String name) {
        return mContext.getResources().getStringArray(resource(name, "array"));
    }

    /** The receivers and observers of the original's helpers. */
    private void registerReceivers() {
        UserManagerInternal users = LocalServices.getService(UserManagerInternal.class);
        users.addUserVisibilityListener(
                (userId, visible) -> host(h -> h.onUserVisibilityChanged(userId, visible)));
        PowerManagerInternal power = LocalServices.getService(PowerManagerInternal.class);
        power.registerLowPowerModeObserver(SERVICE_TYPE_LOCATION,
                state -> host(h -> h.onLocationPowerSaveModeChanged(locationMode(state))));
        int mode = locationMode(power.getLowPowerState(SERVICE_TYPE_LOCATION));
        host(h -> h.onLocationPowerSaveModeChanged(mode));
        IntentFilter screen = new IntentFilter();
        screen.addAction(Intent.ACTION_SCREEN_OFF);
        screen.addAction(Intent.ACTION_SCREEN_ON);
        mContext.registerReceiverAsUser(new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                boolean on = Intent.ACTION_SCREEN_ON.equals(intent.getAction());
                host(h -> h.onScreenInteractiveChanged(on));
            }
        }, UserHandle.ALL, screen, null, FgThread.getHandler());
        IntentFilter packages = new IntentFilter();
        packages.addAction(Intent.ACTION_PACKAGE_CHANGED);
        packages.addAction(Intent.ACTION_PACKAGE_REMOVED);
        packages.addAction(Intent.ACTION_PACKAGE_RESTARTED);
        packages.addAction(Intent.ACTION_QUERY_PACKAGE_RESTART);
        packages.addDataScheme("package");
        mContext.registerReceiver(new PackageReceiver(), packages);
    }

    /** PowerManager.getLocationPowerSaveMode of a power save state. */
    private static int locationMode(PowerSaveState state) {
        return state.batterySaverEnabled ? state.locationMode : LOCATION_MODE_NO_CHANGE;
    }

    /** SystemPackageResetHelper's receiver. */
    private final class PackageReceiver extends BroadcastReceiver {
        @Override
        public void onReceive(Context context, Intent intent) {
            String action = intent.getAction();
            Uri data = intent.getData();
            String packageName = data == null ? null : data.getSchemeSpecificPart();
            if (action == null || packageName == null) {
                return;
            }
            switch (action) {
                case Intent.ACTION_QUERY_PACKAGE_RESTART:
                    String[] packages = intent.getStringArrayExtra(Intent.EXTRA_PACKAGES);
                    ILocationHost host = host();
                    if (packages == null || host == null) {
                        return;
                    }
                    for (String p : packages) {
                        try {
                            if (host.isResetableForPackage(p)) {
                                setResultCode(-1); // Activity.RESULT_OK
                                return;
                            }
                        } catch (RemoteException e) {
                            return;
                        }
                    }
                    return;
                case Intent.ACTION_PACKAGE_CHANGED:
                    String[] components =
                            intent.getStringArrayExtra(Intent.EXTRA_CHANGED_COMPONENT_NAME_LIST);
                    boolean packageChanged = false;
                    if (components != null) {
                        for (String c : components) {
                            packageChanged |= packageName.equals(c);
                        }
                    }
                    if (!packageChanged) {
                        return;
                    }
                    try {
                        ApplicationInfo info =
                                context.getPackageManager().getApplicationInfo(packageName, 0);
                        if (info.enabled) {
                            return;
                        }
                    } catch (PackageManager.NameNotFoundException e) {
                        return;
                    }
                    host(h -> h.onPackageReset(packageName));
                    return;
                case Intent.ACTION_PACKAGE_REMOVED:
                case Intent.ACTION_PACKAGE_RESTARTED:
                    host(h -> h.onPackageReset(packageName));
                    return;
                default:
                    return;
            }
        }
    }

    /** A bound provider's ServiceWatcher listener. */
    private final class Listener implements ServiceWatcher.ServiceListener<BoundServiceInfo> {
        private final String mProvider;

        Listener(String provider) {
            mProvider = provider;
        }

        @Override
        public void onBind(IBinder binder, BoundServiceInfo info) {
            Bundle metadata = info.getMetadata();
            String tags = metadata == null ? null : metadata.getString(EXTRA_LOCATION_TAGS);
            String packageName = info.getComponentName().getPackageName();
            synchronized (mLock) {
                mBound.put(mProvider, new Object[] {mProvider, binder, packageName, tags});
            }
            host(h -> h.onProviderBound(mProvider, binder, packageName, tags));
        }

        @Override
        public void onUnbind() {
            synchronized (mLock) {
                mBound.remove(mProvider);
            }
            host(h -> h.onProviderUnbound(mProvider));
        }
    }

    private interface HostCall {
        void call(ILocationHost host) throws RemoteException;
    }

    private ILocationHost host() {
        synchronized (mLock) {
            return mHost;
        }
    }

    private void host(HostCall call) {
        ILocationHost host = host();
        if (host == null) {
            return;
        }
        try {
            call.call(host);
        } catch (RemoteException e) {
            Slog.w(TAG, "the location host died", e);
        }
    }

    // ILocationBridge

    @Override
    public void registerContentObserver(String uri, boolean notifyForDescendants,
            IBinder observer, int userId) {
        enforceSystemUid();
        final long identity = Binder.clearCallingIdentity();
        try {
            ContentResolver.getContentService().registerContentObserver(Uri.parse(uri),
                    notifyForDescendants, IContentObserver.Stub.asInterface(observer), userId,
                    mContext.getApplicationInfo().targetSdkVersion);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public String[] getUnthrottledLocationPackages() {
        enforceSystemUid();
        Set<String> packages = SystemConfig.getInstance().getAllowUnthrottledLocation();
        return packages.toArray(new String[0]);
    }

    @Override
    public String[] getLocationSettingsAllowlist(boolean ignore) {
        enforceSystemUid();
        SystemConfig config = SystemConfig.getInstance();
        Map<String, ? extends Set<String>> map = ignore
                ? config.getAllowIgnoreLocationSettings()
                : config.getAllowAdasLocationSettings();
        ArrayList<String> entries = new ArrayList<>();
        for (Map.Entry<String, ? extends Set<String>> e : map.entrySet()) {
            StringBuilder entry = new StringBuilder(e.getKey());
            for (String tag : e.getValue()) {
                entry.append(';').append(tag == null ? "null" : tag);
            }
            entries.add(entry.toString());
        }
        return entries.toArray(new String[0]);
    }

    @Override
    public void invalidateLocationEnabledCache() {
        enforceSystemUid();
        LocationManager.invalidateLocalLocationEnabledCaches();
    }

    @Override
    public void setProviderStarted(String provider, boolean started) {
        enforceSystemUid();
        ServiceWatcher watcher;
        synchronized (mLock) {
            watcher = mWatchers.get(provider);
        }
        if (watcher == null) {
            return;
        }
        final long identity = Binder.clearCallingIdentity();
        try {
            if (started) {
                watcher.register();
            } else {
                watcher.unregister();
            }
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public void setLocationPackageTags(int uid, String[] packageTags) {
        enforceSystemUid();
        PackageTagsList.Builder builder = new PackageTagsList.Builder();
        for (String entry : packageTags) {
            String[] parts = entry.split(";");
            for (int i = 1; i < parts.length; i++) {
                builder.add(parts[0], "null".equals(parts[i]) ? null : parts[i]);
            }
        }
        PackageTagsList tags = builder.build();
        LocationManagerInternal.LocationPackageTagsListener listener;
        synchronized (mLock) {
            mTags.put(uid, tags);
            listener = mTagsListener;
        }
        if (listener != null) {
            FgThread.getHandler().post(() -> listener.onLocationPackageTagsChanged(uid, tags));
        }
    }

    private static void enforceSystemUid() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("the bridge serves the system uid only");
        }
    }

    /**
     * LocationManagerInternal: the native service's answers, through its
     * binder and its host.
     */
    private final class Internal extends LocationManagerInternal {
        private final Map<ProviderEnabledListener, BroadcastReceiver> mEnabledListeners =
                new HashMap<>();

        private ILocationManager service() {
            return ILocationManager.Stub.asInterface(
                    ServiceManager.getService(Context.LOCATION_SERVICE));
        }

        @Override
        public boolean isProviderEnabledForUser(String provider, int userId) {
            try {
                return service().isProviderEnabledForUser(provider, userId);
            } catch (RemoteException e) {
                throw e.rethrowFromSystemServer();
            }
        }

        @Override
        public void addProviderEnabledListener(String provider,
                ProviderEnabledListener listener) {
            BroadcastReceiver receiver = new BroadcastReceiver() {
                @Override
                public void onReceive(Context context, Intent intent) {
                    if (provider.equals(
                            intent.getStringExtra(LocationManager.EXTRA_PROVIDER_NAME))) {
                        listener.onProviderEnabledChanged(provider, getSendingUserId(),
                                intent.getBooleanExtra(LocationManager.EXTRA_PROVIDER_ENABLED,
                                        false));
                    }
                }
            };
            synchronized (mEnabledListeners) {
                mEnabledListeners.put(listener, receiver);
            }
            mContext.registerReceiverAsUser(receiver, UserHandle.ALL,
                    new IntentFilter(LocationManager.PROVIDERS_CHANGED_ACTION), null,
                    FgThread.getHandler());
        }

        @Override
        public void removeProviderEnabledListener(String provider,
                ProviderEnabledListener listener) {
            BroadcastReceiver receiver;
            synchronized (mEnabledListeners) {
                receiver = mEnabledListeners.remove(listener);
            }
            if (receiver != null) {
                mContext.unregisterReceiver(receiver);
            }
        }

        @Override
        public boolean isProvider(String provider, CallerIdentity identity) {
            ILocationHost host = host();
            if (host == null) {
                return false;
            }
            try {
                return host.isProvider(provider, identity.getUid(), identity.getPid(),
                        identity.getPackageName(), identity.getAttributionTag());
            } catch (RemoteException e) {
                return false;
            }
        }

        @Override
        public LocationTime getGnssTimeMillis() {
            try {
                return service().getGnssTimeMillis();
            } catch (RemoteException e) {
                throw e.rethrowFromSystemServer();
            }
        }

        @Override
        public void setLocationPackageTagsListener(LocationPackageTagsListener listener) {
            Map<Integer, PackageTagsList> tags;
            synchronized (mLock) {
                mTagsListener = listener;
                tags = new HashMap<>(mTags);
            }
            if (listener == null) {
                return;
            }
            for (Map.Entry<Integer, PackageTagsList> e : tags.entrySet()) {
                if (!e.getValue().isEmpty()) {
                    FgThread.getHandler().post(
                            () -> listener.onLocationPackageTagsChanged(e.getKey(), e.getValue()));
                }
            }
        }
    }
}
