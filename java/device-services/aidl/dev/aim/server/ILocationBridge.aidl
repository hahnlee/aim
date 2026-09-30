package dev.aim.server;

/**
 * What the native location service (crates/aim-services, ADR 0013) needs
 * from system_server (docs/system-services.md, "The system_server
 * bridge"): each method is the binder form of what the original
 * LocationManagerService does in system_server. Only the service host
 * holds it, and it answers only the system uid.
 */
interface ILocationBridge {
    /**
     * IContentService.registerContentObserver as system_server's own:
     * `observer` (an IContentObserver) hears of changes to `uri` for
     * `userId`.
     */
    void registerContentObserver(String uri, boolean notifyForDescendants,
            IBinder observer, int userId);

    /**
     * SystemConfig's allow-unthrottled-location packages.
     */
    String[] getUnthrottledLocationPackages();

    /**
     * SystemConfig's allow-ignore-location-settings (`ignore`) or
     * allow-adas-location-settings packages, each as `package;tag;...`
     * (`*` for every tag, `null` for none).
     */
    String[] getLocationSettingsAllowlist(boolean ignore);

    /** LocationManager.invalidateLocalLocationEnabledCaches(). */
    void invalidateLocationEnabledCache();

    /**
     * Starts or stops watching a bound provider's service
     * (ServiceWatcher.register, unregister), as a test provider in its
     * place stops the real one.
     */
    void setProviderStarted(String provider, boolean started);

    /**
     * Tells LocationManagerInternal's package tags listener (AppOpsPolicy)
     * of the location providers' attribution tags of `uid`, each as
     * `package;tag;...` (`null` for none).
     */
    void setLocationPackageTags(int uid, in String[] packageTags);
}
