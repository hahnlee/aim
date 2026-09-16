package dev.darwinart.runtime.storage;

import android.os.Binder;
import android.os.Environment;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.os.UserHandle;
import android.os.storage.StorageVolume;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.io.File;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.util.UUID;

/** System-server owner for the Android 16 IStorageManager volume contract. */
public final class StorageManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.os.storage.IStorageManager";
    private static final int PRIMARY_USER = 0;

    private final int getVolumeListCode = transaction("getVolumeList");
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
        if (code != getVolumeListCode || reply == null) {
            return super.onTransact(code, data, reply, flags);
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
