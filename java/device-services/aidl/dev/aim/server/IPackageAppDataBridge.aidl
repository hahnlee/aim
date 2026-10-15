package dev.aim.server;
/** Original installd and UM/storage owners; inode claims protect rollback. */
interface IPackageAppDataBridge {
    int appDataFlags(int userId);
    byte[] createAppData(String volumeUuid, String packageName, int userId, int flags,
            int appId, String seInfo, int targetSdkVersion);
    void commitAppData(String packageName, int userId, long ceDataInode);
    void rollbackAppData(String packageName, int userId, long ceDataInode);
}
