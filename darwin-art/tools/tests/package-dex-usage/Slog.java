package android.util;
public final class Slog {
    public static int e(String tag, String message) { System.err.println(tag + ": " + message); return 0; }
    public static int e(String tag, String message, Throwable error) { error.printStackTrace(); return e(tag, message); }
    public static int w(String tag, String message) { return e(tag, message); }
    public static int w(String tag, String message, Throwable error) { return e(tag, message, error); }
    public static int wtf(String tag, String message) { return e(tag, message); }
}
