package android.util;

/** Test stub. */
public final class Log {
    public static int errors;
    public static int e(String tag, String message) { errors++; return 0; }
}
