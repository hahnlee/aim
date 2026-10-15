package dev.aim.server;

/** Original StorageManager owner used by native package startability. */
public final class PackageLifecycleLeaf extends IPackageLifecycleLeaf.Stub {
    @Override public boolean isCeStorageUnlocked(int userId) {
        if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID) {
            throw new SecurityException("package lifecycle leaf requires native system UID");
        }
        return android.os.storage.StorageManager.isCeStorageUnlocked(userId);
    }
}
