package dev.aim.server;

import dev.aim.server.INotificationPermissionCallback;

/**
 * One app's request for POST_NOTIFICATIONS, which system_server's
 * interceptor hands the request's activity in place of
 * PermissionController's dialog (#470). Holding it is what lets the
 * activity ask for that app, and for that app only.
 */
oneway interface INotificationPermissionRequest {
    /** Asks the Mac through the native service host. */
    void ask(INotificationPermissionCallback callback);
}
