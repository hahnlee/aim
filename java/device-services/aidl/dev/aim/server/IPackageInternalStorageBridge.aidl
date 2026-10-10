package dev.aim.server;
interface IPackageInternalStorageBridge {
    void freeAllAboveQuota(String volume);
    long deleteOat(String packageName, int callerUid);
    void migrateObb();
    String[] writableVolumes();
    boolean ceUnlocked(int user);
    boolean fileEncrypted();
    boolean applyDefaultDeviceStorage();
    void cleanupInvalid(String volume, int user, int flags);
    String[] dataDirectoryNames(String volume, int user, boolean ce);
    void destroyData(String volume, String packageName, int user, int flags, long ceInode);
    long[] prepareData(String volume, String packageName, int user, int flags, int appId, String seinfo, int targetSdk, boolean usesSdk);
    void migrateData(String volume, String packageName, int user, int target);
    void prepareContents(String volume, String packageName, int user, int flags, String primaryAbi, String nativeLibraryDir);
    void fixupData(String volume, int flags);
}
