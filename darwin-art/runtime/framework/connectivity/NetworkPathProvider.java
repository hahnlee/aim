package dev.darwinart.runtime.connectivity;

import java.util.Arrays;

/** Rust-owned macOS Network.framework snapshot exposed as Android cost state. */
public final class NetworkPathProvider implements ConnectivityState, AutoCloseable {
    private static final int STATUS_SATISFIED = 1;
    private static final int FLAG_EXPENSIVE = 1 << 0;
    private static final int FLAG_CONSTRAINED = 1 << 1;
    private static final int MAX_DNS_SERVERS = 8;

    private long handle;
    private ConnectivitySnapshot previous;
    private long previousNativeGeneration = Long.MIN_VALUE;
    private long generation;

    public NetworkPathProvider() {
        handle = nativeCreate();
        if (handle == 0) {
            throw new IllegalStateException("macOS network path monitor did not start");
        }
    }

    private synchronized String[] nativeSnapshot() {
        if (handle == 0) throw new IllegalStateException("network path monitor is closed");
        return nativeSnapshot(handle);
    }

    @Override
    public boolean isActiveNetworkMetered() {
        return snapshot().isMetered();
    }

    @Override
    public synchronized ConnectivitySnapshot snapshot() {
        String[] raw = nativeSnapshot();
        if (raw.length < 6) throw new IllegalStateException("invalid network path snapshot");
        boolean satisfied = parseUnsigned(raw[0]) == STATUS_SATISFIED;
        int flags = (int) parseUnsigned(raw[1]);
        int interfaceMask = (int) parseUnsigned(raw[2]);
        long nativeGeneration = parseUnsigned(raw[3]);
        String interfaceName = emptyToNull(raw[4]);
        int dnsCount = Math.min(MAX_DNS_SERVERS, (int) parseUnsigned(raw[5]));
        String[] dnsServers = new String[dnsCount];
        for (int i = 0; i < dnsCount; ++i) {
            int index = 6 + i;
            if (index >= raw.length || raw[index] == null || raw[index].isEmpty()) {
                dnsServers = Arrays.copyOf(dnsServers, i);
                break;
            }
            dnsServers[i] = raw[index];
        }
        if (previous == null || nativeGeneration != previousNativeGeneration
                || previous.hasActiveNetwork() != satisfied
                || previous.isConstrained() != ((flags & FLAG_CONSTRAINED) != 0)
                || previous.interfaceMask() != interfaceMask
                || !same(previous.interfaceName(), interfaceName)
                || !Arrays.equals(previous.dnsServers(), dnsServers)) {
            generation = generation == Long.MAX_VALUE ? 1 : generation + 1;
        }
        previousNativeGeneration = nativeGeneration;
        previous = ConnectivitySnapshot.fromNetworkPath(satisfied,
                (flags & FLAG_EXPENSIVE) != 0, (flags & FLAG_CONSTRAINED) != 0,
                interfaceMask, generation, interfaceName, dnsServers);
        return previous;
    }

    public boolean isConstrained() {
        return snapshot().isConstrained();
    }

    @Override
    public synchronized void close() {
        if (handle == 0) return;
        nativeDestroy(handle);
        handle = 0;
        previous = null;
    }

    private static native long nativeCreate();
    private static native String[] nativeSnapshot(long handle);
    private static native void nativeDestroy(long handle);

    private static long parseUnsigned(String value) {
        try {
            return Long.parseLong(value);
        } catch (RuntimeException error) {
            throw new IllegalStateException("invalid network path snapshot field", error);
        }
    }

    private static String emptyToNull(String value) {
        return value == null || value.isEmpty() ? null : value;
    }

    private static boolean same(String first, String second) {
        return first == null ? second == null : first.equals(second);
    }
}
