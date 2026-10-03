package dev.aim.server;

import android.os.Parcel;
import android.os.RemoteException;
import java.io.IOException;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;

/** Private code transport bound to one native capture, before replica construction. */
public final class PackageScanLease implements AutoCloseable {
    private static final int CHUNK = 64 * 1024;
    private final IPackageScanSnapshot endpoint;
    private final long version;
    private final Map<String, PackageCode> active = new HashMap<>();
    private final Map<String, PackageCode> disabled = new HashMap<>();
    private final Map<String, PackageUsageState> usage = new HashMap<>();
    private boolean closed;

    public PackageScanLease(IPackageScanSnapshot endpoint) throws RemoteException {
        this.endpoint = Objects.requireNonNull(endpoint);
        version = endpoint.getVersion();
    }

    public long getVersion() { return version; }

    public synchronized PackageCode getCode(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        Map<String, PackageCode> cache = factory ? disabled : active;
        if (cache.containsKey(name)) return cache.get(name);
        int length = endpoint.getCodeLength(name, factory);
        if (length == -1) {
            cache.put(name, null);
            return null;
        }
        if (length <= 0) throw new IOException("invalid package code length");
        byte[] bytes = new byte[length];
        int offset = 0;
        while (offset < length) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getCodeChunk(name, factory, offset, requested);
            if (chunk == null || chunk.length != requested) {
                throw new IOException("incomplete package code chunk");
            }
            System.arraycopy(chunk, 0, bytes, offset, requested);
            offset += requested;
        }
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            PackageCode code = PackageCode.CREATOR.createFromParcel(parcel);
            if (parcel.dataAvail() != 0 || code.getVersion() != version
                    || !code.getPackageName().equals(name)) {
                throw new IOException("package code capture mismatch");
            }
            cache.put(name, code);
            return code;
        } finally {
            parcel.recycle();
        }
    }

    public synchronized PackageUsageState getUsage(String name) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        if (usage.containsKey(name)) return usage.get(name);
        byte[] bytes = endpoint.getUsage(name);
        if (bytes == null) {
            usage.put(name, null);
            return null;
        }
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            PackageUsageState state = PackageUsageState.CREATOR.createFromParcel(parcel);
            if (parcel.dataAvail() != 0 || state.getVersion() != version
                    || !state.getPackageName().equals(name)) {
                throw new IOException("package usage capture mismatch");
            }
            usage.put(name, state);
            return state;
        } finally { parcel.recycle(); }
    }

    @Override
    public synchronized void close() throws RemoteException {
        if (closed) return;
        endpoint.close();
        closed = true;
        active.clear();
        disabled.clear();
        usage.clear();
    }
}
