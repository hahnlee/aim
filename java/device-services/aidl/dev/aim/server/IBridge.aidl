package dev.aim.server;

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
}
