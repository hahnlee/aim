package android.os;

/** Test-only monotonic clock standing in for the native SystemClock. */
public final class SystemClock {
    private SystemClock() {}

    public static long uptimeMillis() { return System.nanoTime() / 1_000_000L; }
}
