package com.android.server.content;

import java.io.File;
import java.io.IOException;
import java.nio.channels.FileChannel;
import java.nio.channels.FileLock;
import java.nio.file.StandardOpenOption;

/** Process-lifetime exclusion for the original storage engine and adapter cache. */
public final class SyncStorageOwner {
    private static boolean claimed;
    private final FileChannel channel;
    private final FileLock lock;
    public SyncStorageOwner(File data) {
        synchronized (SyncStorageOwner.class) {
            if (claimed) throw new IllegalStateException("sync owner construction already attempted");
            claimed = true;
        }
        try {
            File directory = new File(data, "system/sync");
            if (!directory.isDirectory() && !directory.mkdirs())
                throw new IOException("cannot create " + directory);
            channel = FileChannel.open(new File(directory, "owner.lock").toPath(),
                    StandardOpenOption.CREATE, StandardOpenOption.WRITE);
            FileLock acquired;
            try {
                acquired = channel.tryLock();
                if (acquired == null) throw new IOException("sync writer already running");
            } catch (IOException | RuntimeException e) {
                channel.close();
                throw e;
            }
            lock = acquired;
        } catch (IOException | RuntimeException e) {
            throw new IllegalStateException("cannot acquire original sync storage/cache owner", e);
        }
    }
}
