package dev.darwinart.runtime.storage;

import android.os.storage.StorageManagerInternal;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The storage manager's local interface, published in LocalServices as
 * AOSP's StorageManagerService does, for the AOSP services this runtime runs.
 * It records which users' credential-encrypted storage the user manager has
 * prepared; every other call reports that the runtime does not provide it.
 */
public final class StorageManagerLocal extends StorageManagerInternal {
    private final Set<Integer> ceStoragePrepared = ConcurrentHashMap.newKeySet();

    private static UnsupportedOperationException unsupported(String method) {
        return new UnsupportedOperationException(
                "StorageManagerInternal." + method + " is not provided by this runtime");
    }

    @Override
    public void addResetListener(android.os.storage.StorageManagerInternal.ResetListener arg0) {
        throw unsupported("addResetListener");
    }

    @Override
    public android.os.IInstalld.IFsveritySetupAuthToken createFsveritySetupAuthToken(android.os.ParcelFileDescriptor arg0, int arg1) throws java.io.IOException {
        throw unsupported("createFsveritySetupAuthToken");
    }

    @Override
    public int enableFsverity(android.os.IInstalld.IFsveritySetupAuthToken arg0, java.lang.String arg1, java.lang.String arg2) throws java.io.IOException {
        throw unsupported("enableFsverity");
    }

    @Override
    public void freeCache(java.lang.String arg0, long arg1) {
        throw unsupported("freeCache");
    }

    @Override
    public int getExternalStorageMountMode(int arg0, java.lang.String arg1) {
        throw unsupported("getExternalStorageMountMode");
    }

    @Override
    public java.util.List<java.lang.String> getPrimaryVolumeIds() {
        throw unsupported("getPrimaryVolumeIds");
    }

    @Override
    public boolean hasExternalStorageAccess(int arg0, java.lang.String arg1) {
        throw unsupported("hasExternalStorageAccess");
    }

    @Override
    public boolean hasLegacyExternalStorage(int arg0) {
        throw unsupported("hasLegacyExternalStorage");
    }

    @Override
    public boolean isCeStoragePrepared(int userId) {
        return ceStoragePrepared.contains(userId);
    }

    @Override
    public boolean isExternalStorageService(int arg0) {
        throw unsupported("isExternalStorageService");
    }

    @Override
    public boolean isFuseMounted(int arg0) {
        throw unsupported("isFuseMounted");
    }

    @Override
    public void markCeStoragePrepared(int userId) {
        ceStoragePrepared.add(userId);
    }

    @Override
    public void onAppOpsChanged(int arg0, int arg1, java.lang.String arg2, int arg3, int arg4) {
        throw unsupported("onAppOpsChanged");
    }

    @Override
    public void prepareAppDataAfterInstall(java.lang.String arg0, int arg1) {
        throw unsupported("prepareAppDataAfterInstall");
    }

    @Override
    public boolean prepareStorageDirs(int arg0, java.util.Set<java.lang.String> arg1, java.lang.String arg2) {
        throw unsupported("prepareStorageDirs");
    }

    @Override
    public void prepareUserStorageForMove(java.lang.String arg0, java.lang.String arg1, java.util.List<android.content.pm.UserInfo> arg2) {
        throw unsupported("prepareUserStorageForMove");
    }

    @Override
    public void registerCloudProviderChangeListener(android.os.storage.StorageManagerInternal.CloudProviderChangeListener arg0) {
        throw unsupported("registerCloudProviderChangeListener");
    }

    @Override
    public void registerStorageLockEventListener(android.os.storage.ICeStorageLockEventListener arg0) {
        throw unsupported("registerStorageLockEventListener");
    }

    @Override
    public void resetUser(int arg0) {
        throw unsupported("resetUser");
    }

    @Override
    public void unregisterStorageLockEventListener(android.os.storage.ICeStorageLockEventListener arg0) {
        throw unsupported("unregisterStorageLockEventListener");
    }
}
