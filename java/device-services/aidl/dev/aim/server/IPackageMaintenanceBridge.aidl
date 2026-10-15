package dev.aim.server;
import android.os.IBinder;
import android.os.ParcelFileDescriptor;
import android.content.IntentSender;
/** Independent original ART owners consuming the native retained package facade. */
interface IPackageMaintenanceBridge {
    IBinder getArtManagerBinder();
    boolean dexoptPackage(String packageName, String compilerFilter, boolean force,
            boolean bootComplete, String splitName, boolean secondaryOnly,
            int callingUid, int callingPid);
    void notifyDexLoad(String loadingPackageName, in String[] dexPaths,
            in String[] classLoaderContexts, String loaderIsa, int callingUid, int callingPid);
    void clearAppProfiles(String packageName);
    boolean checkPermission(String permission, int callingUid, int callingPid);
    void enforcePermission(String permission, int callingUid, int callingPid);
    byte[] storageSpace(String volumeUuid);
    boolean freeCacheV2();
    boolean preloadsExpired();
    long cachePeriod(String setting, long defaultValue);
    long wallTimeMillis();
    void deletePreloadsFileCache();
    void deleteParserCache();
    void freeInstalldCache(String volumeUuid, long targetBytes, boolean v2, boolean defyQuota);
    void freeStorageServiceCache(String volumeUuid, long requiredBytes);
    void noteCacheClearCaller(String packageName, int callingUid, int callingPid);
    void sendFreeStorageResult(in IntentSender sender, boolean success);
    void clearCacheFiles(String packageName, int userId, boolean canAccessInstantApps,
            int callingUid);
    /** Original ART shell command; actual caller is preserved by the native Binder owner. */
    int handleArtShellCommand(in ParcelFileDescriptor input, in ParcelFileDescriptor output,
            in ParcelFileDescriptor error, in String[] arguments);
}
