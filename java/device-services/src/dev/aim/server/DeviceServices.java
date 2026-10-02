package dev.aim.server;

import android.app.WindowConfiguration;
import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.os.Binder;
import android.os.Environment;
import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.SystemProperties;
import android.util.Slog;
import android.view.IWindowManager;

import com.android.internal.os.ApplicationSharedMemory;
import com.android.server.SystemService;
import com.android.server.compat.PlatformCompat;
import com.android.server.pm.parsing.library.PackageBackwardCompatibility;

import java.io.File;
import java.io.IOException;

/**
 * The device's own system service, which SystemServer starts from
 * {@code config_deviceSpecificSystemServices} (the framework overlay of
 * java/framework-overlay), after the system services are ready. It
 * publishes nothing: at its first boot phase it hands the native service
 * host (ADR 0013) the bridge to system_server's internal state
 * (docs/system-services.md, "The system_server bridge").
 * In the lightweight shell it also starts the window shell (WindowShell,
 * docs/task-organizer.md), which aim-windows finds as aim.window_shell.
 */
public final class DeviceServices extends SystemService {
    private static final String TAG = "AimDeviceServices";

    /** The native service host's name in servicemanager. */
    private static final String SERVICE_HOST = "aim.service_host";
    /** WindowManager's name in servicemanager (Context.WINDOW_SERVICE). */
    private static final String WINDOW = "window";
    /** Display.DEFAULT_DISPLAY. */
    private static final int DEFAULT_DISPLAY = 0;

    private final LocationBridge mLocation;
    private final UiModeBridge mUiMode;
    private final LocaleBridge mLocale = new LocaleBridge();
    private final PackageFeed mPackageFeed;
    private final PackageWrites mPackageWrites;

    public DeviceServices(Context context) {
        super(context);
        mLocation = new LocationBridge(context);
        mUiMode = new UiModeBridge(context);
        mPackageFeed = new PackageFeed(context);
        mPackageWrites = new PackageWrites(context);
    }

    @Override
    public void onStart() {
        mLocation.publish();
    }

    @Override
    public void onBootPhase(int phase) {
        if (phase == PHASE_DEVICE_SPECIFIC_SERVICES_READY) {
            // Before the home activity starts: every task is organized.
            if (SystemProperties.getBoolean(WindowShell.PROPERTY, false)) {
                fullscreenDisplayArea();
                WindowShell.start(getContext());
            }
            attachBridge();
        } else if (phase == PHASE_THIRD_PARTY_APPS_CAN_START) {
            mLocation.onThirdPartyAppsCanStart();
            mLocale.onThirdPartyAppsCanStart();
        }
    }

    @Override
    public void onUserStarting(TargetUser user) {
        mLocation.onUserStarting(user.getUserIdentifier());
    }

    @Override
    public void onUserUnlocking(TargetUser user) {
        if (SystemProperties.getBoolean(WindowShell.PROPERTY, false)) {
            dropFreeformLaunchParams(user.getUserIdentifier());
        }
    }

    @Override
    public void onUserSwitching(TargetUser from, TargetUser to) {
        mLocation.onUserSwitching(from.getUserIdentifier(), to.getUserIdentifier());
        mUiMode.onUserSwitching(from.getUserIdentifier(), to.getUserIdentifier());
    }

    @Override
    public void onUserStopped(TargetUser user) {
        mLocation.onUserStopped(user.getUserIdentifier());
    }

    /**
     * Makes the default display area fullscreen, as the window shell has it
     * (docs/task-organizer.md, section 2). WindowManager keeps a display's
     * windowing mode in its display settings, and a boot from before the
     * lightweight shell left it freeform there; WindowManager applies it to
     * the display area once ActivityTaskManager knows freeform is
     * supported, after this phase, so a check here reads fullscreen
     * whatever is stored. Setting it stores fullscreen, before the home's
     * first task could record launch params (#636).
     */
    private static void fullscreenDisplayArea() {
        IWindowManager wm = IWindowManager.Stub.asInterface(ServiceManager.getService(WINDOW));
        try {
            wm.setWindowingMode(DEFAULT_DISPLAY, WindowConfiguration.WINDOWING_MODE_FULLSCREEN);
        } catch (RemoteException e) {
            Slog.w(TAG, "cannot make the default display area fullscreen", e);
        }
    }

    /**
     * Drops the launch params WindowManager recorded for `userId` while the
     * default display area was freeform, before it reads them once the user
     * is unlocked (#636): they would lay a running task out again and
     * cascade it (#613). Under the window shell the display area is
     * fullscreen and nothing records there (docs/task-organizer.md,
     * section 2), so only the first boot of a data directory from before
     * the lightweight shell finds any.
     */
    private static void dropFreeformLaunchParams(int userId) {
        File dir = new File(Environment.getDataSystemCeDirectory(userId), "launch_params");
        File[] records = dir.listFiles();
        if (records == null || records.length == 0) {
            return;
        }
        for (File record : records) {
            record.delete();
        }
        Slog.i(TAG, "dropped " + records.length + " launch params of a freeform display area");
    }

    private void attachBridge() {
        IBinder host = ServiceManager.checkService(SERVICE_HOST);
        if (host == null) {
            Slog.i(TAG, "no native service host");
            return;
        }
        try {
            IServiceHost service = IServiceHost.Stub.asInterface(host);
            service.attachBridge(new Bridge(getContext(), service, mLocation, mUiMode, mLocale,
                    mPackageFeed, mPackageWrites));
            Slog.i(TAG, "bridge attached to the native service host");
        } catch (RemoteException e) {
            Slog.w(TAG, "cannot attach the bridge", e);
        }
    }

    private static final class Bridge extends IBridge.Stub {
        // SharedLibrariesImpl, android-16.0.0_r1.
        private static final long ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES = 142191088L;
        private final Context context;
        private final IServiceHost host;
        private final LocationBridge location;
        private final UiModeBridge uiMode;
        private final LocaleBridge locale;
        private final PackageFeed packageFeed;
        private final PackageWrites packageWrites;

        Bridge(Context context, IServiceHost host, LocationBridge location, UiModeBridge uiMode,
                LocaleBridge locale, PackageFeed packageFeed, PackageWrites packageWrites) {
            this.context = context;
            this.host = host;
            this.location = location;
            this.uiMode = uiMode;
            this.locale = locale;
            this.packageFeed = packageFeed;
            this.packageWrites = packageWrites;
        }

        @Override
        public ParcelFileDescriptor getApplicationSharedMemory() {
            enforceSystemUid();
            try {
                return new ParcelFileDescriptor(
                        ApplicationSharedMemory.getInstance().getReadOnlyFileDescriptor());
            } catch (IOException e) {
                throw new IllegalStateException(e);
            }
        }

        @Override
        public void interceptNotificationPermissionRequests() {
            enforceSystemUid();
            NotificationPermissionInterceptor.register(context, host);
        }

        @Override
        public ILocationBridge getLocationBridge(ILocationHost host) {
            enforceSystemUid();
            return location.attach(host);
        }

        @Override
        public IUiModeBridge getUiModeBridge(IUiModeHost host) {
            enforceSystemUid();
            return uiMode.attach(host);
        }

        @Override
        public void updateLocales(String languageTags) {
            enforceSystemUid();
            locale.update(languageTags);
        }

        @Override
        public IPackageFeed getPackageFeed(IPackageFeedHost host) {
            enforceSystemUid();
            return packageFeed.attach(host);
        }

        @Override
        public void watchPackageWrites(IPackageWritesHost host) {
            enforceSystemUid();
            packageWrites.attach(host);
        }

        @Override
        public boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk) {
            enforceSystemUid();
            return platformCompat().isChangeEnabledInternal(
                    ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES, packageName, targetSdk);
        }

        @Override
        public boolean isTestBaseOnBootclasspath() {
            enforceSystemUid();
            return PackageBackwardCompatibility.bootClassPathContainsATB();
        }

        @Override
        public boolean isPackageChangeEnabled(long changeId, byte[] applicationInfo) {
            enforceSystemUid();
            if (applicationInfo == null) throw new IllegalArgumentException("missing ApplicationInfo");
            Parcel parcel = Parcel.obtain();
            try {
                parcel.unmarshall(applicationInfo, 0, applicationInfo.length);
                parcel.setDataPosition(0);
                ApplicationInfo info = ApplicationInfo.CREATOR.createFromParcel(parcel);
                parcel.enforceNoDataAvail();
                return platformCompat().isChangeEnabled(changeId, info);
            } finally {
                parcel.recycle();
            }
        }

        private static PlatformCompat platformCompat() {
            PlatformCompat compat = (PlatformCompat) ServiceManager.getService(
                    Context.PLATFORM_COMPAT_SERVICE);
            if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
            return compat;
        }

        private static void enforceSystemUid() {
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("the bridge serves the system uid only");
            }
        }
    }
}
