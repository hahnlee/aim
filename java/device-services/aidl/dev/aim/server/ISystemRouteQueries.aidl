package dev.aim.server;

/** Cold MediaRouter2 queries. Explicit identities are supplied only by the system UID. */
interface ISystemRouteQueries {
    byte[] getSystemRoutes(int uid, int pid, String callerPackageName, boolean isProxyRouter);
    byte[] getSystemSessionInfo(int uid, int pid, String callerPackageName,
            String targetPackageName, boolean setDeviceRouteSelected);
}
