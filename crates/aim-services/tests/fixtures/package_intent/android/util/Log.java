package android.util;

public class Log {
    public static int d(String tag, String message) { throw new AssertionError("unexpected log"); }
    public static int d(String tag, String message, Throwable error) { throw new AssertionError("unexpected log", error); }
}
