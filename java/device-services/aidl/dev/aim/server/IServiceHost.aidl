package dev.aim.server;

import dev.aim.server.IBridge;
import dev.aim.server.IPackageScanSnapshot;
import dev.aim.server.IPackageBootstrapBridge;
import dev.aim.server.INotificationPermissionCallback;

/**
 * The native service host (crates/aim-services), registered with
 * servicemanager as `aim.service_host`.
 */
interface IServiceHost {
    /** system_server's bridge, once its system services are ready. */
    oneway void attachBridge(IBridge bridge);

    /**
     * An app's request for POST_NOTIFICATIONS (#470): asks the Mac for
     * the notification authorization of `packageName` (its shim's prompt,
     * the first time), grants or revokes the permission of user `userId`
     * by the answer, and tells `callback`.
     */
    oneway void requestNotificationPermission(String packageName, int userId,
            INotificationPermissionCallback callback);
    /** Synchronous attach before native package scanning; no late listeners. */
    void attachPackageBootstrapBridge(IPackageBootstrapBridge bridge);
    /** Lease on the current complete native package graph; unavailable before publication. */
    IPackageScanSnapshot capturePackageScan();
    /** SDK data filesystem owner; serialized with the native package install lock. */
    void reconcilePackageSdkData(String volumeUuid, String packageName, in List<String> subDirNames,
            int userId, int appId, int previousAppId, String seInfo, int flags);
}
