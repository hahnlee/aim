package dev.aim.server;
interface IPackageInternalEventsBridge {
    byte[] prepareRestarted(String packageName, int uid, int flags);
    byte[] prepareDataCleared(String packageName, int uid, int userId, boolean restore, boolean instant);
    void broadcast(in byte[] intent, int userId, boolean instant, in int[] allowList);
}
