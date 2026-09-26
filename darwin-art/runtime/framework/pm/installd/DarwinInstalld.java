package dev.darwinart.runtime.pm.installd;

import android.os.CreateAppDataArgs;
import android.os.CreateAppDataResult;
import android.os.IInstalld;
import android.os.ParcelFileDescriptor;
import android.os.ReconcileSdkDataArgs;
import android.os.ServiceSpecificException;
import android.os.storage.CrateMetadata;

/**
 * The {@code installd} service for this runtime (ADR 0009).
 *
 * <p>AIDL marshalling is AOSP's {@link IInstalld.Stub}. App data and installed
 * code live in host trees owned by the profile daemon, so those operations
 * are forwarded there. A few calls have a fixed answer on this host: there
 * are no SELinux labels to restore, no per-app host uids to fix up and no
 * filesystem quotas. Everything else fails with EOPNOTSUPP instead of
 * pretending to succeed.</p>
 */
public final class DarwinInstalld extends IInstalld.Stub {
    public static final String SERVICE_NAME = "installd";
    // Android errno values reported in ServiceSpecificException.
    private static final int EINVAL = 22;
    private static final int EIO = 5;
    private static final int EOPNOTSUPP = 95;

    // Return codes of the native transport (installd_jni.cc).
    private static final int TRANSPORT_OK = 0;
    private static final int TRANSPORT_FAILED = 1;

    private static native int nativeCreateAppData(String packageName, int userId, int flags,
            long[] inodes, int[] errno);
    private static native int nativeAppData(String packageName, int userId, int flags,
            boolean clear, int[] errno);
    private static native int nativeRmPackageDir(String codePath, int[] errno);

    private static void check(int status, int[] errno, String operation) {
        if (status == TRANSPORT_OK) return;
        if (status == TRANSPORT_FAILED) {
            throw new ServiceSpecificException(errno[0], operation + " failed");
        }
        throw new ServiceSpecificException(EIO, operation + ": profile daemon unavailable");
    }

    private static void requireInternal(String uuid) {
        if (uuid != null) {
            throw new ServiceSpecificException(EINVAL, "only internal storage exists: " + uuid);
        }
    }

    private static ServiceSpecificException unsupported(String operation) {
        return new ServiceSpecificException(EOPNOTSUPP,
                "installd." + operation + " is not provided by this runtime");
    }

    @Override
    public CreateAppDataResult createAppData(CreateAppDataArgs args) {
        CreateAppDataResult result = new CreateAppDataResult();
        try {
            requireInternal(args.uuid);
            long[] inodes = new long[2];
            int[] errno = new int[1];
            check(nativeCreateAppData(args.packageName, args.userId, args.flags, inodes, errno),
                    errno, "createAppData " + args.packageName);
            result.ceDataInode = inodes[0];
            result.deDataInode = inodes[1];
        } catch (ServiceSpecificException error) {
            // createAppDataBatched reports failures per package.
            result.exceptionCode = error.errorCode;
            result.exceptionMessage = error.getMessage();
        }
        return result;
    }

    @Override
    public CreateAppDataResult[] createAppDataBatched(CreateAppDataArgs[] args) {
        CreateAppDataResult[] results = new CreateAppDataResult[args.length];
        for (int i = 0; i < args.length; i++) results[i] = createAppData(args[i]);
        return results;
    }

    @Override
    public void destroyAppData(String uuid, String packageName, int userId, int flags,
            long ceDataInode) {
        requireInternal(uuid);
        int[] errno = new int[1];
        check(nativeAppData(packageName, userId, flags, false, errno), errno,
                "destroyAppData " + packageName);
    }

    @Override
    public void clearAppData(String uuid, String packageName, int userId, int flags,
            long ceDataInode) {
        requireInternal(uuid);
        int[] errno = new int[1];
        check(nativeAppData(packageName, userId, flags, true, errno), errno,
                "clearAppData " + packageName);
    }

    @Override
    public void rmPackageDir(String packageName, String packageDir) {
        int[] errno = new int[1];
        check(nativeRmPackageDir(packageDir, errno), errno, "rmPackageDir " + packageName);
    }

    // Fixed answers on this host.

    /** There are no SELinux labels on the host filesystem. */
    @Override
    public void restoreconAppData(String uuid, String packageName, int userId, int flags,
            int appId, String seInfo) {
        requireInternal(uuid);
    }

    /**
     * Every app tree belongs to the single host user; Android uid isolation is
     * the per-app filesystem namespace, so there is no ownership to repair.
     */
    @Override
    public void fixupAppData(String uuid, int flags) {
        requireInternal(uuid);
    }

    /** No quota-enabled mounts exist to rescan. */
    @Override
    public void invalidateMounts() {}

    @Override
    public boolean isQuotaSupported(String uuid) {
        return false;
    }

    /** installd's first-boot work is quota project setup, which does not apply. */
    @Override
    public void setFirstBoot() {}

    // Not provided.

    /**
     * installd createUserData: ensure_config_user_dirs creates the user's
     * /data/misc/user/<userId> (0750) for device-encrypted internal storage.
     */
    @Override public void createUserData(String uuid, int userId, int userSerial, int flags) {
        if (userId < 0) throw new IllegalArgumentException("Invalid user " + userId);
        if ((flags & android.os.storage.StorageManager.FLAG_STORAGE_DE) == 0 || uuid != null) {
            return;
        }
        java.io.File config = new java.io.File("/data/misc/user/" + userId);
        if (!config.isDirectory() && !config.mkdirs()) {
            throw new android.os.ServiceSpecificException(
                    android.system.OsConstants.EIO, "Failed to ensure dirs for " + userId);
        }
        try {
            android.system.Os.chmod(config.getPath(), 0750);
        } catch (android.system.ErrnoException error) {
            throw new android.os.ServiceSpecificException(error.errno,
                    "Failed to ensure dirs for " + userId);
        }
    }
    @Override public void destroyUserData(String uuid, int userId, int flags) {
        throw unsupported("destroyUserData");
    }
    @Override public void reconcileSdkData(ReconcileSdkDataArgs args) {
        throw unsupported("reconcileSdkData");
    }
    @Override public void migrateAppData(String uuid, String packageName, int userId, int flags) {
        throw unsupported("migrateAppData");
    }
    @Override public long[] getAppSize(String uuid, String[] packageNames, int userId, int flags,
            int appId, long[] ceDataInodes, String[] codePaths) {
        throw unsupported("getAppSize");
    }
    @Override public long[] getUserSize(String uuid, int userId, int flags, int[] appIds) {
        throw unsupported("getUserSize");
    }
    @Override public long[] getExternalSize(String uuid, int userId, int flags, int[] appIds) {
        throw unsupported("getExternalSize");
    }
    @Override public CrateMetadata[] getAppCrates(String uuid, String[] packageNames, int userId) {
        throw unsupported("getAppCrates");
    }
    @Override public CrateMetadata[] getUserCrates(String uuid, int userId) {
        throw unsupported("getUserCrates");
    }
    @Override public void setAppQuota(String uuid, int userId, int appId, long cacheQuota) {
        throw unsupported("setAppQuota");
    }
    @Override public void moveCompleteApp(String fromUuid, String toUuid, String packageName,
            int appId, String seInfo, int targetSdkVersion, String fromCodePath) {
        throw unsupported("moveCompleteApp");
    }
    @Override public boolean dexopt(String apkPath, int uid, String packageName,
            String instructionSet, int dexoptNeeded, String outputPath, int dexFlags,
            String compilerFilter, String uuid, String sharedLibraries, String seInfo,
            boolean downgrade, int targetSdkVersion, String profileName,
            String dexMetadataPath, String compilationReason) {
        throw unsupported("dexopt");
    }
    @Override public void controlDexOptBlocking(boolean block) {
        throw unsupported("controlDexOptBlocking");
    }
    @Override public void rmdex(String codePath, String instructionSet) {
        throw unsupported("rmdex");
    }
    @Override public int mergeProfiles(int uid, String packageName, String profileName) {
        throw unsupported("mergeProfiles");
    }
    @Override public boolean dumpProfiles(int uid, String packageName, String profileName,
            String codePath, boolean dumpClassesAndMethods) {
        throw unsupported("dumpProfiles");
    }
    @Override public boolean copySystemProfile(String systemProfile, int uid, String packageName,
            String profileName) {
        throw unsupported("copySystemProfile");
    }
    @Override public void clearAppProfiles(String packageName, String profileName) {
        throw unsupported("clearAppProfiles");
    }
    @Override public void destroyAppProfiles(String packageName) {
        throw unsupported("destroyAppProfiles");
    }
    @Override public void deleteReferenceProfile(String packageName, String profileName) {
        throw unsupported("deleteReferenceProfile");
    }
    @Override public boolean createProfileSnapshot(int appId, String packageName,
            String profileName, String classpath) {
        throw unsupported("createProfileSnapshot");
    }
    @Override public void destroyProfileSnapshot(String packageName, String profileName) {
        throw unsupported("destroyProfileSnapshot");
    }
    @Override public void freeCache(String uuid, long targetFreeBytes, int flags) {
        throw unsupported("freeCache");
    }
    @Override public void linkNativeLibraryDirectory(String uuid, String packageName,
            String nativeLibPath32, int userId) {
        throw unsupported("linkNativeLibraryDirectory");
    }
    @Override public void createOatDir(String packageName, String oatDir, String instructionSet) {
        throw unsupported("createOatDir");
    }
    @Override public void linkFile(String packageName, String relativePath, String fromBase,
            String toBase) {
        throw unsupported("linkFile");
    }
    @Override public void moveAb(String packageName, String apkPath, String instructionSet,
            String outputPath) {
        throw unsupported("moveAb");
    }
    @Override public long deleteOdex(String packageName, String apkPath, String instructionSet,
            String outputPath) {
        throw unsupported("deleteOdex");
    }
    @Override public boolean reconcileSecondaryDexFile(String dexPath, String pkgName, int uid,
            String[] isas, String volumeUuid, int storageFlag) {
        throw unsupported("reconcileSecondaryDexFile");
    }
    @Override public byte[] hashSecondaryDexFile(String dexPath, String pkgName, int uid,
            String volumeUuid, int storageFlag) {
        throw unsupported("hashSecondaryDexFile");
    }
    @Override public boolean prepareAppProfile(String packageName, int userId, int appId,
            String profileName, String codePath, String dexMetadata) {
        throw unsupported("prepareAppProfile");
    }
    @Override public long snapshotAppData(String uuid, String packageName, int userId,
            int snapshotId, int storageFlags) {
        throw unsupported("snapshotAppData");
    }
    @Override public void restoreAppDataSnapshot(String uuid, String packageName, int appId,
            String seInfo, int user, int snapshotId, int storageflags) {
        throw unsupported("restoreAppDataSnapshot");
    }
    @Override public void destroyAppDataSnapshot(String uuid, String packageName, int userId,
            long ceSnapshotInode, int snapshotId, int storageFlags) {
        throw unsupported("destroyAppDataSnapshot");
    }
    @Override public void destroyCeSnapshotsNotSpecified(String uuid, int userId,
            int[] retainSnapshotIds) {
        throw unsupported("destroyCeSnapshotsNotSpecified");
    }
    @Override public void tryMountDataMirror(String volumeUuid) {
        throw unsupported("tryMountDataMirror");
    }
    @Override public void onPrivateVolumeRemoved(String volumeUuid) {
        throw unsupported("onPrivateVolumeRemoved");
    }
    @Override public void migrateLegacyObbData() {
        throw unsupported("migrateLegacyObbData");
    }
    @Override public void cleanupInvalidPackageDirs(String uuid, int userId, int flags) {
        throw unsupported("cleanupInvalidPackageDirs");
    }
    @Override public int getOdexVisibility(String packageName, String apkPath,
            String instructionSet, String outputPath) {
        throw unsupported("getOdexVisibility");
    }
    @Override public IInstalld.IFsveritySetupAuthToken createFsveritySetupAuthToken(
            ParcelFileDescriptor authFd, int uid) {
        throw unsupported("createFsveritySetupAuthToken");
    }
    @Override public int enableFsverity(IInstalld.IFsveritySetupAuthToken authToken,
            String filePath, String packageName) {
        throw unsupported("enableFsverity");
    }
}
