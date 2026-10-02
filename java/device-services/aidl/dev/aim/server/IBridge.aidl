package dev.aim.server;

import dev.aim.server.ILocationBridge;
import dev.aim.server.ILocationHost;
import dev.aim.server.IPackageFeed;
import dev.aim.server.IPackageFeedHost;
import dev.aim.server.IPackageWritesHost;
import dev.aim.server.IUiModeBridge;
import dev.aim.server.IUiModeHost;

/**
 * What the native system services need from system_server's internals
 * (docs/system-services.md, "The system_server bridge"): each method is
 * the binder form of one internal call. Only the service host holds it,
 * and it answers only the system uid.
 */
interface IBridge {
    /**
     * A read-only fd of ActivityManager's ApplicationSharedMemory, which
     * holds the framework's cache nonces (#497).
     */
    ParcelFileDescriptor getApplicationSharedMemory();

    /**
     * Sends apps' requests for POST_NOTIFICATIONS alone to the Mac's
     * prompt, for a host that shows notifications on the Mac (#470).
     */
    void interceptNotificationPermissionRequests();

    /**
     * The location bridge (ILocationBridge), for `host`, the native
     * location service's side of it.
     */
    ILocationBridge getLocationBridge(ILocationHost host);

    /**
     * The uimode bridge (IUiModeBridge), for `host`, the native uimode
     * service's side of it.
     */
    IUiModeBridge getUiModeBridge(IUiModeHost host);

    /**
     * LocalePicker.updateLocales: the device's languages become
     * `languageTags` (LocaleList.forLanguageTags), the Mac's preferred
     * languages, as Settings' language page sets a choice.
     */
    void updateLocales(String languageTags);

    /**
     * The package feed (IPackageFeed) to `host`, the native PackageManager's
     * model: a snapshot of the original's state now, then the changes.
     */
    IPackageFeed getPackageFeed(IPackageFeedHost host);

    /**
     * Tells `host`, the native PackageManager's write model, of the
     * original's install sessions from now on (IPackageWritesHost).
     */
    void watchPackageWrites(IPackageWritesHost host);

    /** PlatformCompat's install-time native shared-library policy. */
    boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk);

}
