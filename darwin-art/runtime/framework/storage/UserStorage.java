package dev.darwinart.runtime.storage;

import android.os.storage.StorageManager;
import android.system.ErrnoException;
import android.system.Os;
import java.io.File;
import java.io.IOException;

/**
 * vold's per-user storage preparation (fscrypt_prepare_user_storage) for the
 * internal volume, as the system server sees /data. The host keeps no
 * per-user encryption keys, so only the directories and their modes are
 * prepared; ownership is the process owner's.
 */
final class UserStorage {
    private UserStorage() {}

    private static final class Directory {
        final String format;
        final int mode;

        Directory(String format, int mode) {
            this.format = format;
            this.mode = mode;
        }
    }

    // Utils.cpp Build*Path and FsCrypt.cpp prepare_dir modes, in vold's order.
    private static final Directory[] DEVICE_ENCRYPTED = {
        new Directory("/data/system/users/%d", 0700),
        new Directory("/data/misc/profiles/cur/%d", 0771),
        new Directory("/data/system_de/%d", 0770),
        new Directory("/data/vendor_de/%d", 0771),
        new Directory("/data/misc_de/%d", 01771),
        new Directory("/data/user_de/%d", 0771),
    };
    private static final Directory[] CREDENTIAL_ENCRYPTED = {
        new Directory("/data/system_ce/%d", 0770),
        new Directory("/data/vendor_ce/%d", 0771),
        new Directory("/data/media/%d", 02770),
        new Directory("/data/misc_ce/%d", 01771),
        new Directory("/data/user/%d", 0771),
    };

    static void prepare(String volumeUuid, int userId, int flags) throws IOException {
        if (volumeUuid != null) {
            // Only the internal volume exists; there is no adopted storage.
            throw new IllegalArgumentException("Unknown volume " + volumeUuid);
        }
        if (userId < 0) throw new IllegalArgumentException("Invalid user " + userId);
        if ((flags & StorageManager.FLAG_STORAGE_DE) != 0) prepare(DEVICE_ENCRYPTED, userId);
        if ((flags & StorageManager.FLAG_STORAGE_CE) != 0) prepare(CREDENTIAL_ENCRYPTED, userId);
    }

    private static void prepare(Directory[] directories, int userId) throws IOException {
        for (Directory directory : directories) {
            File path = new File(String.format(directory.format, userId));
            if (!path.isDirectory() && !path.mkdirs()) {
                throw new IOException("Failed to prepare " + path);
            }
            try {
                Os.chmod(path.getPath(), directory.mode);
            } catch (ErrnoException error) {
                throw new IOException("Failed to set mode of " + path, error);
            }
        }
    }
}
