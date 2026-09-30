package dev.aim.server;

/**
 * The answer to an app's request for POST_NOTIFICATIONS, sent to the
 * request's activity (java/notification-permission, #470).
 */
oneway interface INotificationPermissionCallback {
    /** The permission's state after the user answered the Mac's prompt. */
    void onResult(boolean granted);

    /**
     * The request is filtered as PermissionController filters it: no
     * prompt and no result.
     */
    void onIgnored();
}
