package android.os;

/** Test-only monotonic clock backed by the host JVM monotonic source. */
public final class SystemClock {
    private SystemClock() {}
    public static long elapsedRealtime() { return System.nanoTime() / 1000000L; }
}
