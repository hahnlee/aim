package dev.aim.server;

import android.app.ActivityTaskManager;
import android.app.ActivityManagerInternal;
import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.StatusBarManager;
import android.app.UiModeManager;
import android.content.BroadcastReceiver;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.content.res.Resources;
import android.database.IContentObserver;
import android.net.Uri;
import android.os.Binder;
import android.os.IBinder;
import android.os.PowerManager;
import android.os.PowerManagerInternal;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.SystemProperties;
import android.os.UserHandle;
import android.service.dreams.DreamManagerInternal;
import android.service.dreams.Sandman;
import android.service.vr.IVrManager;
import android.service.vr.IVrStateCallbacks;
import android.util.Slog;

import com.android.server.FgThread;
import com.android.server.LocalServices;
import com.android.server.UiModeManagerInternal;
import com.android.server.twilight.TwilightListener;
import com.android.server.twilight.TwilightManager;
import com.android.server.twilight.TwilightState;
import com.android.server.wm.ActivityTaskManagerInternal;
import com.android.server.wm.WindowManagerInternal;

/**
 * The system_server side of the native uimode service (ADR 0013,
 * docs/system-services.md): what the original UiModeManagerService does
 * inside system_server, for the service host. It tells the host of docking,
 * charging, power save, twilight, the screen, the clock, restored settings,
 * shutdown, VR and users, and applies what the host decides: the global
 * configuration, an app's night mode, the client caches, the car mode's
 * broadcasts, notification and status bar, dock apps and dreams, and the
 * wake lock of car and desk mode.
 */
final class UiModeBridge extends IUiModeBridge.Stub {
    private static final String TAG = "AimUiModeBridge";
    /** UiModeManagerService's TAG, which names its wake lock. */
    private static final String UI_MODE_TAG = "UiModeManager";
    /** PowerManager.ServiceType.NIGHT_MODE. */
    private static final int SERVICE_TYPE_NIGHT_MODE = 16;
    /** SystemMessage.NOTE_CAR_MODE_DISABLE and SystemNotificationChannels.CAR_MODE. */
    private static final int NOTE_CAR_MODE_DISABLE = 10;
    private static final String CHANNEL_CAR_MODE = "CAR_MODE";
    /** ActivityManager's START_SUCCESS codes and START_INTENT_NOT_RESOLVED. */
    private static final int FIRST_START_SUCCESS_CODE = 0;
    private static final int LAST_START_SUCCESS_CODE = 99;
    private static final int START_INTENT_NOT_RESOLVED = -91;
    /** BatteryManager.EXTRA_PLUGGED. */
    private static final String EXTRA_PLUGGED = "plugged";
    /** Activity.RESULT_OK. */
    private static final int RESULT_OK = -1;

    private final Context mContext;
    private final Object mLock = new Object();
    private IUiModeHost mHost;
    private boolean mReceiversRegistered;
    private PowerManager.WakeLock mWakeLock;
    private TwilightManager mTwilightManager;
    private final TwilightListener mTwilightListener =
            state -> host(h -> h.onTwilightStateChanged(twilight(state)));

    UiModeBridge(Context context) {
        mContext = context;
    }

    /**
     * IBridge.getUiModeBridge: the host of the native uimode service; none
     * while the original runs (it publishes UiModeManagerInternal).
     */
    IUiModeBridge attach(IUiModeHost host) {
        if (LocalServices.getService(UiModeManagerInternal.class) != null) {
            return null;
        }
        synchronized (mLock) {
            mHost = host;
            if (!mReceiversRegistered) {
                mReceiversRegistered = true;
                registerReceivers();
            }
        }
        return this;
    }

    void onUserSwitching(int fromUserId, int toUserId) {
        host(h -> h.onUserSwitching(fromUserId, toUserId));
    }

    /** The receivers, observer and listeners of onBootPhase. */
    private void registerReceivers() {
        mWakeLock = mContext.getSystemService(PowerManager.class)
                .newWakeLock(PowerManager.FULL_WAKE_LOCK, UI_MODE_TAG);
        mTwilightManager = LocalServices.getService(TwilightManager.class);
        PowerManagerInternal power = LocalServices.getService(PowerManagerInternal.class);
        boolean saver = power.getLowPowerState(SERVICE_TYPE_NIGHT_MODE).batterySaverEnabled;
        host(h -> h.onPowerSaveChanged(saver));
        power.registerLowPowerModeObserver(SERVICE_TYPE_NIGHT_MODE,
                state -> host(h -> h.onPowerSaveChanged(state.batterySaverEnabled)));
        IVrManager vr = IVrManager.Stub.asInterface(ServiceManager.getService(Context.VR_SERVICE));
        if (vr != null) {
            try {
                vr.registerListener(new IVrStateCallbacks.Stub() {
                    @Override
                    public void onVrStateChanged(boolean enabled) {
                        host(h -> h.onVrStateChanged(enabled));
                    }
                });
            } catch (RemoteException e) {
                Slog.e(TAG, "Failed to register VR mode state listener: " + e);
            }
        }
        receive(new IntentFilter(Intent.ACTION_DOCK_EVENT), intent -> host(h -> h.onDockEvent(
                intent.getIntExtra(Intent.EXTRA_DOCK_STATE, Intent.EXTRA_DOCK_STATE_UNDOCKED))));
        receive(new IntentFilter(Intent.ACTION_BATTERY_CHANGED), intent -> host(h ->
                h.onBatteryChanged(intent.getIntExtra(EXTRA_PLUGGED, 0) != 0)));
        receive(new IntentFilter(Intent.ACTION_SETTING_RESTORED), intent -> host(h ->
                h.onSettingRestored(intent.getStringExtra(Intent.EXTRA_SETTING_NAME))));
        receive(new IntentFilter(Intent.ACTION_SHUTDOWN), intent -> host(h -> h.onShutdown()));
        IntentFilter inactive = new IntentFilter(Intent.ACTION_SCREEN_OFF);
        inactive.addAction(Intent.ACTION_DREAMING_STARTED);
        receive(inactive, intent -> host(h -> h.onDeviceInactive()));
        IntentFilter time = new IntentFilter(Intent.ACTION_TIME_CHANGED);
        time.addAction(Intent.ACTION_TIMEZONE_CHANGED);
        receive(time, intent -> host(h -> h.onTimeChanged()));
    }

    private interface OnReceive {
        void onReceive(Intent intent);
    }

    private void receive(IntentFilter filter, OnReceive onReceive) {
        mContext.registerReceiver(new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                onReceive.onReceive(intent);
            }
        }, filter);
    }

    /** A TwilightState as the host takes it. */
    private static int twilight(TwilightState state) {
        return state == null ? -1 : state.isNight() ? 1 : 0;
    }

    private interface HostCall {
        void call(IUiModeHost host) throws RemoteException;
    }

    private void host(HostCall call) {
        IUiModeHost host;
        synchronized (mLock) {
            host = mHost;
        }
        if (host == null) {
            return;
        }
        try {
            call.call(host);
        } catch (RemoteException e) {
            Slog.w(TAG, "the uimode host died", e);
        }
    }

    private int integer(Resources res, String name) {
        return res.getInteger(res.getIdentifier(name, "integer", "android"));
    }

    private int bool(Resources res, String name) {
        return res.getBoolean(res.getIdentifier(name, "bool", "android")) ? 1 : 0;
    }

    // IUiModeBridge

    @Override
    public int[] getConfig() {
        enforceSystemUid();
        Resources res = mContext.getResources();
        PackageManager pm = mContext.getPackageManager();
        return new int[] {
            integer(res, "config_defaultUiModeType"),
            integer(res, "config_carDockKeepsScreenOn"),
            integer(res, "config_deskDockKeepsScreenOn"),
            bool(res, "config_startDreamImmediatelyOnDock"),
            bool(res, "config_dreamsDisabledByAmbientModeSuppressionConfig"),
            bool(res, "config_enableCarDockHomeLaunch"),
            bool(res, "config_lockUiMode"),
            pm.hasSystemFeature(PackageManager.FEATURE_TELEVISION)
                    || pm.hasSystemFeature(PackageManager.FEATURE_LEANBACK) ? 1 : 0,
            pm.hasSystemFeature(PackageManager.FEATURE_AUTOMOTIVE) ? 1 : 0,
            pm.hasSystemFeature(PackageManager.FEATURE_WATCH) ? 1 : 0,
            android.view.accessibility.Flags.forceInvertColor() ? 1 : 0,
        };
    }

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
    public void updateConfiguration(int uiMode) {
        enforceSystemUid();
        // Load the splash screen instead of a snapshot.
        LocalServices.getService(WindowManagerInternal.class).clearSnapshotCache();
        Configuration configuration = configuration(uiMode);
        final long identity = Binder.clearCallingIdentity();
        try {
            ActivityTaskManager.getService().updateConfiguration(configuration);
        } catch (RemoteException e) {
            Slog.w(TAG, "Failure communicating with activity manager", e);
        } catch (SecurityException e) {
            Slog.e(TAG, "Activity does not have the ", e);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    /** UiModeManagerService's mConfiguration: the defaults, with `uiMode`. */
    private static Configuration configuration(int uiMode) {
        Configuration configuration = new Configuration();
        configuration.setToDefaults();
        configuration.uiMode = uiMode;
        return configuration;
    }

    @Override
    public void setApplicationNightMode(int pid, int uid, int configNightMode) {
        enforceSystemUid();
        // createPackageConfigurationUpdater() finds the calling process
        // by its pid; this one is the host's, so find the app's.
        String packageName =
                LocalServices.getService(ActivityManagerInternal.class).getPackageNameByPid(pid);
        if (packageName == null) {
            Slog.w(TAG, "setApplicationNightMode: cannot find pid " + pid);
            return;
        }
        LocalServices.getService(ActivityTaskManagerInternal.class)
                .createPackageConfigurationUpdater(packageName, UserHandle.getUserId(uid))
                .setNightMode(configNightMode)
                .commit();
    }

    @Override
    public void invalidateNightModeCache() {
        enforceSystemUid();
        if (android.app.Flags.enableNightModeBinderCache()) {
            UiModeManager.invalidateNightModeCache();
        }
    }

    @Override
    public void invalidateCurrentModeTypeCache() {
        enforceSystemUid();
        if (android.app.Flags.enableCurrentModeTypeBinderCache()) {
            UiModeManager.invalidateCurrentModeTypeCache();
        }
    }

    @Override
    public void setDeviceTheme(String theme) {
        enforceSystemUid();
        SystemProperties.set("persist.sys.theme", theme);
    }

    @Override
    public boolean isDeviceActive() {
        enforceSystemUid();
        return mContext.getSystemService(PowerManager.class).isInteractive()
                && !LocalServices.getService(DreamManagerInternal.class).isDreaming();
    }

    @Override
    public void setTwilightListening(boolean listening) {
        enforceSystemUid();
        if (mTwilightManager == null) {
            return;
        }
        if (listening) {
            mTwilightManager.registerListener(mTwilightListener, FgThread.getHandler());
        } else {
            mTwilightManager.unregisterListener(mTwilightListener);
        }
    }

    @Override
    public int getTwilightState() {
        enforceSystemUid();
        return mTwilightManager == null ? -1 : twilight(mTwilightManager.getLastTwilightState());
    }

    @Override
    public String dumpTwilightState() {
        enforceSystemUid();
        return mTwilightManager == null
                ? null : String.valueOf(mTwilightManager.getLastTwilightState());
    }

    @Override
    public void sendCarModeBroadcast(boolean enabled, int priority, String packageName) {
        enforceSystemUid();
        Intent intent = new Intent(enabled
                ? UiModeManager.ACTION_ENTER_CAR_MODE_PRIORITIZED
                : UiModeManager.ACTION_EXIT_CAR_MODE_PRIORITIZED);
        intent.putExtra(UiModeManager.EXTRA_CALLING_PACKAGE, packageName);
        intent.putExtra(UiModeManager.EXTRA_PRIORITY, priority);
        final long identity = Binder.clearCallingIdentity();
        try {
            mContext.sendBroadcastAsUser(intent, UserHandle.ALL,
                    android.Manifest.permission.HANDLE_CAR_MODE_CHANGES);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public void sendForegroundBroadcastToAllUsers(String action) {
        enforceSystemUid();
        final long identity = Binder.clearCallingIdentity();
        try {
            mContext.sendBroadcastAsUser(new Intent(action)
                    .addFlags(Intent.FLAG_RECEIVER_FOREGROUND), UserHandle.ALL);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public void sendDockBroadcast(String action, int enableFlags, int disableFlags) {
        enforceSystemUid();
        Intent intent = new Intent(action);
        intent.putExtra("enableFlags", enableFlags);
        intent.putExtra("disableFlags", disableFlags);
        intent.addFlags(Intent.FLAG_RECEIVER_FOREGROUND);
        final long identity = Binder.clearCallingIdentity();
        try {
            mContext.sendOrderedBroadcastAsUser(intent, UserHandle.CURRENT, null,
                    new BroadcastReceiver() {
                        @Override
                        public void onReceive(Context context, Intent result) {
                            int code = getResultCode();
                            host(h -> h.onDockBroadcastResult(result.getAction(),
                                    result.getIntExtra("enableFlags", 0),
                                    result.getIntExtra("disableFlags", 0), code));
                        }
                    }, null, RESULT_OK, null, null);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public void setCarModeStatusBar(boolean carMode) {
        enforceSystemUid();
        final long identity = Binder.clearCallingIdentity();
        try {
            StatusBarManager statusBar = mContext.getSystemService(StatusBarManager.class);
            if (statusBar != null) {
                statusBar.disable(carMode
                        ? StatusBarManager.DISABLE_NOTIFICATION_TICKER
                        : StatusBarManager.DISABLE_NONE);
            }
            NotificationManager notifications =
                    mContext.getSystemService(NotificationManager.class);
            if (notifications == null) {
                return;
            }
            if (!carMode) {
                notifications.cancelAsUser(null, NOTE_CAR_MODE_DISABLE, UserHandle.ALL);
                return;
            }
            Resources res = mContext.getResources();
            Intent carModeOffIntent = new Intent().setClassName("android",
                    "com.android.internal.app.DisableCarModeActivity");
            Notification.Builder n = new Notification.Builder(mContext, CHANNEL_CAR_MODE)
                    .setSmallIcon(res.getIdentifier("stat_notify_car_mode", "drawable", "android"))
                    .setDefaults(Notification.DEFAULT_LIGHTS)
                    .setOngoing(true)
                    .setWhen(0)
                    .setColor(mContext.getColor(res.getIdentifier(
                            "system_notification_accent_color", "color", "android")))
                    .setContentTitle(mContext.getString(res.getIdentifier(
                            "car_mode_disable_notification_title", "string", "android")))
                    .setContentText(mContext.getString(res.getIdentifier(
                            "car_mode_disable_notification_message", "string", "android")))
                    .setContentIntent(PendingIntent.getActivityAsUser(mContext, 0,
                            carModeOffIntent, PendingIntent.FLAG_MUTABLE, null,
                            UserHandle.CURRENT));
            notifications.notifyAsUser(null, NOTE_CAR_MODE_DISABLE, n.build(), UserHandle.ALL);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public boolean startDockApp(String category, int uiMode) {
        enforceSystemUid();
        Intent homeIntent = new Intent(Intent.ACTION_MAIN);
        homeIntent.addCategory(category);
        homeIntent.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                | Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED);
        final long identity = Binder.clearCallingIdentity();
        try {
            if (!Sandman.shouldStartDockApp(mContext, homeIntent)) {
                return false;
            }
            int result = ActivityTaskManager.getService().startActivityWithConfig(
                    null, mContext.getBasePackageName(), mContext.getAttributionTag(),
                    homeIntent, null, null, null, 0, 0, configuration(uiMode), null,
                    UserHandle.USER_CURRENT);
            if (FIRST_START_SUCCESS_CODE <= result && result <= LAST_START_SUCCESS_CODE) {
                return true;
            }
            if (result != START_INTENT_NOT_RESOLVED) {
                Slog.e(TAG, "Could not start dock app: " + homeIntent
                        + ", startActivityWithConfig result " + result);
            }
        } catch (RemoteException ex) {
            Slog.e(TAG, "Could not start dock app: " + homeIntent, ex);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
        return false;
    }

    @Override
    public void startDreamIfDocked(boolean startImmediately,
            boolean disabledByAmbientModeSuppression) {
        enforceSystemUid();
        final long identity = Binder.clearCallingIdentity();
        try {
            if (disabledByAmbientModeSuppression && LocalServices
                    .getService(PowerManagerInternal.class).isAmbientDisplaySuppressed()) {
                return;
            }
            if (startImmediately || LocalServices.getService(WindowManagerInternal.class)
                    .isKeyguardShowingAndNotOccluded()
                    || !mContext.getSystemService(PowerManager.class).isInteractive()) {
                Sandman.startDreamWhenDockedIfAppropriate(mContext);
            }
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    @Override
    public void setKeepScreenOn(boolean on) {
        enforceSystemUid();
        synchronized (mLock) {
            if (on != mWakeLock.isHeld()) {
                if (on) {
                    mWakeLock.acquire();
                } else {
                    mWakeLock.release();
                }
            }
        }
    }

    @Override
    public boolean isSystemUiInDarkTheme() {
        enforceSystemUid();
        int nightMode = android.app.ActivityThread.currentActivityThread().getSystemUiContext()
                .getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK;
        return nightMode == Configuration.UI_MODE_NIGHT_YES;
    }

    private static void enforceSystemUid() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("the bridge serves the system uid only");
        }
    }
}
