package android.util;
public final class Log {
    public static int refusals;
    public static int e(String tag, String message) { refusals++; return 0; }
    public static int w(String tag, String message) { refusals++; return 0; }
}
