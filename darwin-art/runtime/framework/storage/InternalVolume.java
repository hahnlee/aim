package dev.darwinart.runtime.storage;

import android.os.Environment;
import android.os.Parcel;
import android.os.Parcelable;
import java.io.File;

/**
 * The internal private volume (/data) as StorageManagerService reports it:
 * its VolumeInfo and the bytes an allocation may take from it.
 */
final class InternalVolume {
    private InternalVolume() {}

    /**
     * StorageManagerService.getAllocatableBytes without quota support (the
     * installd provider reports none): usable bytes above the low-storage
     * reserve, or above the full reserve for an aggressive allocation.
     */
    static long allocatableBytes(String volumeUuid, int flags) {
        if (volumeUuid != null && !android.os.storage.StorageManager.UUID_PRIVATE_INTERNAL
                .equals(volumeUuid) && !"primary_physical".equals(volumeUuid)
                && !android.os.storage.StorageManager.UUID_DEFAULT.toString().equals(volumeUuid)) {
            throw new IllegalArgumentException("Unknown volume " + volumeUuid);
        }
        File data = Environment.getDataDirectory();
        android.content.Context system =
                android.app.ActivityThread.currentActivityThread().getSystemContext();
        android.os.storage.StorageManager storage =
                system.getSystemService(android.os.storage.StorageManager.class);
        long reserved = (flags & android.os.storage.StorageManager.FLAG_ALLOCATE_AGGRESSIVE) != 0
                ? storage.getStorageFullBytes(data) : storage.getStorageLowBytes(data);
        return Math.max(0, data.getUsableSpace() - reserved);
    }

    /**
     * StorageManagerService.addInternalVolumeLocked: the private internal
     * volume, mounted at /data. vold reports no adoptable or public volumes
     * on this host.
     */
    static android.os.storage.VolumeInfo internalVolume() {
        android.os.storage.VolumeInfo internal = new android.os.storage.VolumeInfo(
                android.os.storage.VolumeInfo.ID_PRIVATE_INTERNAL,
                android.os.storage.VolumeInfo.TYPE_PRIVATE, null, null);
        internal.state = android.os.storage.VolumeInfo.STATE_MOUNTED;
        internal.path = Environment.getDataDirectory().getAbsolutePath();
        return internal;
    }

    /** getVolumes: vold reports only the internal volume on this host. */
    static void writeVolumes(Parcel reply) {
        reply.writeTypedArray(new android.os.storage.VolumeInfo[] {internalVolume()},
                Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
    }

    /** allocateBytes with too little allocatable space: an IOException. */
    static void writeAllocationFailure(Parcel reply, long requested, long allocatable) {
        reply.writeException(new android.os.ParcelableException(new java.io.IOException(
                "Failed to allocate " + requested + " bytes; " + allocatable + " allocatable")));
    }
}
