package dev.aim.server;

import android.os.ParcelFileDescriptor;
import java.io.IOException;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.VarHandle;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.channels.FileChannel;
import java.util.Objects;

/** Native package owner publication counter; one acquire load per replica lookup. */
public final class PackageVersionPage implements PackageSnapshots.VersionSource, AutoCloseable {
    private static final VarHandle VERSION = MethodHandles.byteBufferViewVarHandle(
            long[].class, ByteOrder.LITTLE_ENDIAN);
    private ByteBuffer mapping;

    public PackageVersionPage(ParcelFileDescriptor descriptor) throws IOException {
        Objects.requireNonNull(descriptor);
        try (var stream = new ParcelFileDescriptor.AutoCloseInputStream(descriptor)) {
            var channel = stream.getChannel();
            if (channel.size() < Long.BYTES)
                throw new IOException("native package version page is too short");
            mapping = channel.map(FileChannel.MapMode.READ_ONLY, 0, Long.BYTES);
        }
    }

    @Override
    public synchronized long currentVersion() {
        if (mapping == null) throw new IllegalStateException("package version page is closed");
        long version = (long) VERSION.getAcquire(mapping, 0);
        if (version < 0) throw new IllegalStateException("invalid native package state version");
        return version;
    }

    @Override
    public synchronized void close() {
        mapping = null;
    }
}
