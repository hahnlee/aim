package dev.aim.server;

import dev.aim.server.IBridge;
import dev.aim.server.INotificationPermissionCallback;

/**
 * The native service host (crates/aim-services), registered with
 * servicemanager as `aim.service_host`.
 */
oneway interface IServiceHost {
    /** system_server's bridge, once its system services are ready. */
    void attachBridge(IBridge bridge);

    /**
     * An app's request for POST_NOTIFICATIONS (#470): asks the Mac for
     * the notification authorization of `packageName` (its shim's prompt,
     * the first time), grants or revokes the permission of user `userId`
     * by the answer, and tells `callback`.
     */
    void requestNotificationPermission(String packageName, int userId,
            INotificationPermissionCallback callback);
}
