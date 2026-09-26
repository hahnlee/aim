package dev.darwinart.runtime.storage;

import android.os.Binder;
import android.os.Environment;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.Process;
import android.os.RemoteException;
import android.os.UserHandle;
import android.os.storage.StorageVolume;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.io.File;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

/** System-server owner for the Android 16 IStorageManager volume contract. */
public final class StorageManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.os.storage.IStorageManager";
    private static final int PRIMARY_USER = 0;

    private final int getVolumeListCode = transaction("getVolumeList");
    private final int unlockCeStorageCode = transaction("unlockCeStorage");
    private final int lockCeStorageCode = transaction("lockCeStorage");
    private final int isCeStorageUnlockedCode = transaction("isCeStorageUnlocked");
    private final int prepareUserStorageCode = transaction("prepareUserStorage");
    private final int getAllocatableBytesCode = transaction("getAllocatableBytes");
    private final int allocateBytesCode = transaction("allocateBytes");
    private final int getVolumesCode = transaction("getVolumes");
    private final int needsCheckpointCode = transaction("needsCheckpoint");
    private final int supportsCheckpointCode = transaction("supportsCheckpoint");
    private final int registerListenerCode = transaction("registerListener");
    private final int unregisterListenerCode = transaction("unregisterListener");
    // StorageManagerService.mCallbacks: notified of volume and storage state
    // changes (none happen yet: the one emulated volume stays mounted).
    private final android.os.RemoteCallbackList<android.os.storage.IStorageEventListener>
            listeners = new android.os.RemoteCallbackList<>();
    /**
     * Users whose credential-encrypted storage is unlocked. This host keeps
     * no per-user CE keys, so there is no secret for vold to check: unlocking
     * is the user lifecycle's decision alone, as for an unsecured user.
     */
    private final Set<Integer> ceUnlockedUsers = ConcurrentHashMap.newKeySet();
    private final ApplicationProcessRegistry processes;
    private final String primaryPath;

    /**
     * The mount service reports Android's guest-visible path. Host storage is
     * prepared separately by the launch boundary and is never exposed here.
     */
    public StorageManagerEndpoint(ApplicationProcessRegistry applicationProcesses) {
        this(applicationProcesses, "/storage/emulated/0");
    }

    StorageManagerEndpoint(ApplicationProcessRegistry applicationProcesses, String path) {
        if (applicationProcesses == null) throw new NullPointerException("applicationProcesses");
        processes = applicationProcesses;
        primaryPath = requirePrimaryPath(path);
        attachInterface(null, DESCRIPTOR);
    }

    static String requirePrimaryPath(String path) {
        if (path == null || path.isEmpty() || !path.startsWith("/")) {
            throw new IllegalArgumentException("primary storage path must be absolute");
        }
        return path;
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.os.storage.IStorageManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    /** Builds the one mounted emulated volume exposed by this runtime. */
    static StorageVolume primaryStorageVolume(String path) {
        try {
            Constructor<StorageVolume> constructor = StorageVolume.class.getDeclaredConstructor(
                    String.class, File.class, File.class, String.class,
                    boolean.class, boolean.class, boolean.class, boolean.class,
                    boolean.class, long.class, UserHandle.class, UUID.class,
                    String.class, String.class);
            constructor.setAccessible(true);
            File root = new File(path);
            return constructor.newInstance(
                    "emulated;0", root, root, "Internal shared storage",
                    Boolean.TRUE, Boolean.FALSE, Boolean.TRUE, Boolean.FALSE,
                    Boolean.FALSE, Long.valueOf(0L), android.os.Process.myUserHandle(),
                    null, null, Environment.MEDIA_MOUNTED);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android StorageVolume ABI mismatch", error);
        }
    }

    String primaryPath() {
        return primaryPath;
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (reply != null && (code == unlockCeStorageCode || code == lockCeStorageCode)) {
            data.enforceInterface(DESCRIPTOR);
            int userId = data.readInt();
            if (code == unlockCeStorageCode) data.createByteArray();
            data.enforceNoDataAvail();
            // StorageManagerService requires STORAGE_INTERNAL: the system.
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("CE storage state is owned by the system");
            }
            if (code == unlockCeStorageCode) ceUnlockedUsers.add(userId);
            else ceUnlockedUsers.remove(userId);
            reply.writeNoException();
            return true;
        }
        if (reply != null && (code == getAllocatableBytesCode || code == allocateBytesCode)) {
            data.enforceInterface(DESCRIPTOR);
            String volumeUuid = data.readString();
            long requested = code == allocateBytesCode ? data.readLong() : 0;
            int allocationFlags = data.readInt();
            data.readString(); // calling package
            data.enforceNoDataAvail();
            long allocatable = InternalVolume.allocatableBytes(volumeUuid, allocationFlags);
            if (code == allocateBytesCode && allocatable < requested) {
                // No cached data to free: the volume lacks the space.
                InternalVolume.writeAllocationFailure(reply, requested, allocatable);
                return true;
            }
            reply.writeNoException();
            if (code == getAllocatableBytesCode) reply.writeLong(allocatable);
            return true;
        }
        if (reply != null && code == prepareUserStorageCode) {
            data.enforceInterface(DESCRIPTOR);
            String volumeUuid = data.readString();
            int userId = data.readInt();
            int storageFlags = data.readInt();
            data.enforceNoDataAvail();
            // StorageManagerService requires STORAGE_INTERNAL: the system.
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("User storage is prepared by the system");
            }
            try {
                UserStorage.prepare(volumeUuid, userId, storageFlags);
            } catch (java.io.IOException error) {
                throw new IllegalStateException(error);
            }
            reply.writeNoException();
            return true;
        }
        if (reply != null && code == getVolumesCode) {
            data.enforceInterface(DESCRIPTOR);
            data.readInt(); // flags
            data.enforceNoDataAvail();
            reply.writeNoException();
            InternalVolume.writeVolumes(reply);
            return true;
        }
        if (reply != null && (code == needsCheckpointCode || code == supportsCheckpointCode)) {
            // vold's userdata checkpointing (Virtual A/B updates): this
            // profile's /data is a sparse image without update checkpoints.
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeBoolean(false);
            return true;
        }
        if (code == registerListenerCode || code == unregisterListenerCode) {
            data.enforceInterface(DESCRIPTOR);
            android.os.storage.IStorageEventListener listener =
                    android.os.storage.IStorageEventListener.Stub.asInterface(
                            data.readStrongBinder());
            data.enforceNoDataAvail();
            if (listener != null) {
                if (code == registerListenerCode) {
                    listeners.register(listener);
                } else {
                    listeners.unregister(listener);
                }
            }
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (reply != null && code == isCeStorageUnlockedCode) {
            data.enforceInterface(DESCRIPTOR);
            int userId = data.readInt();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeBoolean(ceUnlockedUsers.contains(userId));
            return true;
        }
        if (code != getVolumeListCode || reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        int userId = data.readInt();
        String callingPackage = data.readString();
        data.readInt(); // flags; this runtime has one mounted emulated volume.
        data.enforceNoDataAvail();

        String attachedPackage = processes.requireIdentifiedProcess(Binder.getCallingPid());
        if (userId != PRIMARY_USER || callingPackage == null
                || !attachedPackage.equals(callingPackage)) {
            throw new SecurityException("Storage caller does not own requested package");
        }

        reply.writeNoException();
        reply.writeTypedArray(new StorageVolume[] {primaryStorageVolume(primaryPath)},
                Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        return true;
    }
}
