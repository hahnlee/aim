package android.util;

public final class Log {
    public static int lifetimeWarnings;
    public static int e(String tag, String message) { return 0; }
    public static int e(String tag, String message, Throwable error) { return 0; }
    public static int w(String tag, String message) { return 0; }
    public static int w(String tag, String message, Throwable error) {
        ++lifetimeWarnings;
        return 0;
    }
}
