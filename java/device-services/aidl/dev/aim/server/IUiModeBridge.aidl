package dev.aim.server;

/**
 * What the native uimode service (crates/aim-services, ADR 0013) needs
 * from system_server (docs/system-services.md, "The uimode service"): each
 * method is the binder form of what the original UiModeManagerService does
 * inside system_server. Only the service host holds it, and it answers
 * only the system uid.
 */
interface IUiModeBridge {
    /**
     * The original's configuration, from the image's resources, features
     * and flags, in this order: config_defaultUiModeType,
     * config_carDockKeepsScreenOn, config_deskDockKeepsScreenOn, and as 0
     * or 1 config_startDreamImmediatelyOnDock,
     * config_dreamsDisabledByAmbientModeSuppressionConfig,
     * config_enableCarDockHomeLaunch, config_lockUiMode,
     * FEATURE_TELEVISION or FEATURE_LEANBACK, FEATURE_AUTOMOTIVE,
     * FEATURE_WATCH, and the flag forceInvertColor.
     */
    int[] getConfig();

    /**
     * IContentService.registerContentObserver as system_server's own:
     * `observer` (an IContentObserver) hears of changes to `uri` for
     * `userId`.
     */
    void registerContentObserver(String uri, boolean notifyForDescendants,
            IBinder observer, int userId);

    /**
     * applyConfigurationExternallyLocked: the snapshot cache cleared, and
     * ActivityTaskManager's configuration updated to `uiMode`.
     */
    void updateConfiguration(int uiMode);

    /**
     * setApplicationNightMode's PackageConfigurationUpdater, for the
     * application of process `pid`, of `uid`.
     */
    void setApplicationNightMode(int pid, int uid, int configNightMode);

    /** UiModeManager.invalidateNightModeCache(), under its flag. */
    void invalidateNightModeCache();

    /** UiModeManager.invalidateCurrentModeTypeCache(), under its flag. */
    void invalidateCurrentModeTypeCache();

    /** SystemProperties.set("persist.sys.theme", theme). */
    void setDeviceTheme(String theme);

    /** PowerManager.isInteractive() and not DreamManagerInternal.isDreaming(). */
    boolean isDeviceActive();

    /**
     * TwilightManager's listener registered or unregistered: its changes
     * go to IUiModeHost.onTwilightStateChanged.
     */
    void setTwilightListening(boolean listening);

    /**
     * TwilightManager.getLastTwilightState(): -1 without one, 1 at night,
     * 0 by day.
     */
    int getTwilightState();

    /** getLastTwilightState() as dumpImpl prints it. */
    String dumpTwilightState();

    /**
     * ACTION_ENTER_CAR_MODE_PRIORITIZED or ACTION_EXIT_CAR_MODE_PRIORITIZED
     * to all users, for HANDLE_CAR_MODE_CHANGES.
     */
    void sendCarModeBroadcast(boolean enabled, int priority, String packageName);

    /** sendForegroundBroadcastToAllUsers(action). */
    void sendForegroundBroadcastToAllUsers(String action);

    /**
     * The ordered broadcast of a dock mode's `action` to the current user,
     * whose result comes to IUiModeHost.onDockBroadcastResult.
     */
    void sendDockBroadcast(String action, int enableFlags, int disableFlags);

    /**
     * adjustStatusBarCarModeLocked: the status bar's notification ticker
     * and the car mode notification.
     */
    void setCarModeStatusBar(boolean carMode);

    /**
     * Starts the dock app of `category` with configuration `uiMode`
     * (startActivityWithConfig), if Sandman lets it; whether it started.
     */
    boolean startDockApp(String category, int uiMode);

    /**
     * Sandman.startDreamWhenDockedIfAppropriate, unless ambient display
     * suppression disables dreams, when the dream starts at once or the
     * keyguard shows or the device is not interactive.
     */
    void startDreamIfDocked(boolean startImmediately, boolean disabledByAmbientModeSuppression);

    /** The full wake lock that keeps the screen on in car or desk mode. */
    void setKeepScreenOn(boolean on);

    /** Whether the system UI context's configuration is in night mode. */
    boolean isSystemUiInDarkTheme();
}
