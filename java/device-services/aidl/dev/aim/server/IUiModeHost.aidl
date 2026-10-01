package dev.aim.server;

/**
 * The native uimode service's side of the bridge (IUiModeBridge), told of
 * what the original UiModeManagerService learns inside system_server.
 */
oneway interface IUiModeHost {
    /** ACTION_DOCK_EVENT's EXTRA_DOCK_STATE. */
    void onDockEvent(int state);

    /** ACTION_BATTERY_CHANGED: whether the device is plugged in. */
    void onBatteryChanged(boolean charging);

    /**
     * PowerManagerInternal's low power mode observer (ServiceType.NIGHT_MODE):
     * whether battery saver is on.
     */
    void onPowerSaveChanged(boolean batterySaverEnabled);

    /** TwilightListener: the state as IUiModeBridge.getTwilightState gives it. */
    void onTwilightStateChanged(int state);

    /** ACTION_SCREEN_OFF or ACTION_DREAMING_STARTED. */
    void onDeviceInactive();

    /** ACTION_TIME_CHANGED or ACTION_TIMEZONE_CHANGED. */
    void onTimeChanged();

    /** ACTION_SETTING_RESTORED of setting `name`. */
    void onSettingRestored(String name);

    /** ACTION_SHUTDOWN. */
    void onShutdown();

    /** IVrStateCallbacks.onVrStateChanged. */
    void onVrStateChanged(boolean enabled);

    /** SystemService.onUserSwitching. */
    void onUserSwitching(int fromUserId, int toUserId);

    /** The result code of a dock mode's ordered broadcast. */
    void onDockBroadcastResult(String action, int enableFlags, int disableFlags, int resultCode);
}
