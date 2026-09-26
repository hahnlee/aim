package android.os;

/** Test stub: fixed memory facts. */
public final class Process {
    public static long getAdvertisedMem() { return 16L << 30; }
    public static long getTotalMemory() { return 16L << 30; }
    public static long freeMemory = 8L << 30;
    public static long getFreeMemory() { return freeMemory; }
}
