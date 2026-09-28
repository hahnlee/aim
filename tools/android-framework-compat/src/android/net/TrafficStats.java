package android.net;

import java.io.FileDescriptor;
import java.io.IOException;
import java.net.DatagramSocket;
import java.net.Socket;
import java.net.SocketException;

/**
 * Host-backed TrafficStats surface for apps which use Android's networking
 * instrumentation hooks.  Darwin has no Android kernel traffic accounting;
 * preserve the API and per-thread tag semantics while making accounting
 * queries report the platform's documented unsupported value.
 */
public final class TrafficStats {
    public static final int UNSUPPORTED = -1;
    private static final ThreadLocal<Integer> THREAD_TAG =
            new ThreadLocal<Integer>() {
                @Override protected Integer initialValue() { return 0; }
            };

    private TrafficStats() {}

    public static void clearThreadStatsTag() { THREAD_TAG.set(0); }
    public static void clearThreadStatsUid() {}
    public static int getAndSetThreadStatsTag(int tag) {
        int previous = getThreadStatsTag();
        setThreadStatsTag(tag);
        return previous;
    }
    public static int getThreadStatsTag() { return THREAD_TAG.get(); }
    public static int getThreadStatsUid() { return UNSUPPORTED; }
    public static void setThreadStatsTag(int tag) { THREAD_TAG.set(tag); }
    public static void setThreadStatsUid(int uid) {}
    public static void incrementOperationCount(int operationCount) {}
    public static void incrementOperationCount(int tag, int operationCount) {}

    public static long getMobileRxBytes() { return UNSUPPORTED; }
    public static long getMobileRxPackets() { return UNSUPPORTED; }
    public static long getMobileTxBytes() { return UNSUPPORTED; }
    public static long getMobileTxPackets() { return UNSUPPORTED; }
    public static long getTotalRxBytes() { return UNSUPPORTED; }
    public static long getTotalRxPackets() { return UNSUPPORTED; }
    public static long getTotalTxBytes() { return UNSUPPORTED; }
    public static long getTotalTxPackets() { return UNSUPPORTED; }
    public static long getRxBytes(String iface) { return UNSUPPORTED; }
    public static long getRxPackets(String iface) { return UNSUPPORTED; }
    public static long getTxBytes(String iface) { return UNSUPPORTED; }
    public static long getTxPackets(String iface) { return UNSUPPORTED; }
    public static long getUidRxBytes(int uid) { return UNSUPPORTED; }
    public static long getUidRxPackets(int uid) { return UNSUPPORTED; }
    public static long getUidTxBytes(int uid) { return UNSUPPORTED; }
    public static long getUidTxPackets(int uid) { return UNSUPPORTED; }
    public static long getUidTcpRxBytes(int uid) { return UNSUPPORTED; }
    public static long getUidTcpRxSegments(int uid) { return UNSUPPORTED; }
    public static long getUidTcpTxBytes(int uid) { return UNSUPPORTED; }
    public static long getUidTcpTxSegments(int uid) { return UNSUPPORTED; }
    public static long getUidUdpRxBytes(int uid) { return UNSUPPORTED; }
    public static long getUidUdpRxPackets(int uid) { return UNSUPPORTED; }
    public static long getUidUdpTxBytes(int uid) { return UNSUPPORTED; }
    public static long getUidUdpTxPackets(int uid) { return UNSUPPORTED; }

    public static void tagSocket(Socket socket) throws SocketException {}
    public static void untagSocket(Socket socket) throws SocketException {}
    public static void tagDatagramSocket(DatagramSocket socket) throws SocketException {}
    public static void untagDatagramSocket(DatagramSocket socket) throws SocketException {}
    public static void tagFileDescriptor(FileDescriptor fd) throws IOException {}
    public static void untagFileDescriptor(FileDescriptor fd) throws IOException {}
}
