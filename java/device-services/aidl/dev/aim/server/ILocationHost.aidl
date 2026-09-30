package dev.aim.server;

/**
 * The native location service's side of the bridge (ILocationBridge),
 * told of what the original LocationManagerService learns inside
 * system_server.
 */
interface ILocationHost {
    /**
     * At PHASE_THIRD_PARTY_APPS_CAN_START: which of the providers bound
     * from apps (`network`, `fused`, `geocoder`, `density`) resolve to a
     * service (ServiceWatcher.checkServiceResolves).
     */
    oneway void onProvidersResolved(in String[] providers);

    /**
     * `provider`'s service bound (ServiceListener.onBind): its binder, its
     * package, and the tags of its `android:location_allow_listed_tags`
     * metadata.
     */
    oneway void onProviderBound(String provider, IBinder binder, String packageName,
            @nullable String extraTags);

    /** `provider`'s service unbound (ServiceListener.onUnbind). */
    oneway void onProviderUnbound(String provider);

    /** SystemService.onUserStarting. */
    oneway void onUserStarting(int userId);

    /** SystemService.onUserStopped. */
    oneway void onUserStopped(int userId);

    /** SystemService.onUserSwitching. */
    oneway void onUserSwitching(int fromUserId, int toUserId);

    /** UserManagerInternal's user visibility listener. */
    oneway void onUserVisibilityChanged(int userId, boolean visible);

    /**
     * PowerManagerInternal's low power mode observer (ServiceType.LOCATION):
     * the location power save mode, LOCATION_MODE_NO_CHANGE without
     * battery saver.
     */
    oneway void onLocationPowerSaveModeChanged(int mode);

    /** ACTION_SCREEN_ON and ACTION_SCREEN_OFF. */
    oneway void onScreenInteractiveChanged(boolean interactive);

    /** A package's state was reset (PackageResetHelper). */
    oneway void onPackageReset(String packageName);

    /**
     * Whether the package has state a force stop would reset
     * (ACTION_QUERY_PACKAGE_RESTART).
     */
    boolean isResetableForPackage(String packageName);

    /** LocationManagerInternal.isProvider(provider, identity). */
    boolean isProvider(@nullable String provider, int uid, int pid, String packageName,
            @nullable String attributionTag);
}
