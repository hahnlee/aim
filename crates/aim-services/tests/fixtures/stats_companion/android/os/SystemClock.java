// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public final class SystemClock {
    public static long elapsed=10000, uptime=9000, thread=500;
    public static long elapsedRealtime() { return elapsed; }
    public static long uptimeMillis() { return uptime; }
    public static long currentThreadTimeMillis() { return thread; }
}
