package dev.aim.server;

import android.content.Context;
import android.content.pm.IPackageMoveObserver;
import android.os.Binder;
import android.os.Process;
import android.os.storage.StorageManager;

/** Original StorageManager drives actual volume migration and progress. */
public final class PackageMoveBridge extends IPackageMoveBridge.Stub {
    private final StorageManager storage;
    public PackageMoveBridge(Context context) {
        storage = java.util.Objects.requireNonNull(context.getSystemService(StorageManager.class));
    }
    @Override
    public void movePrimaryStorage(String volumeUuid, IPackageMoveObserver callback) {
        if (Binder.getCallingUid() != Process.SYSTEM_UID)
            throw new SecurityException("native storage move owner required");
        storage.setPrimaryStorageUuid(volumeUuid, callback);
    }
}
