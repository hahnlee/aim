package android.text;

/** Host-only boundary observer, deliberately performs no text conversion. */
public final class TextUtils {
    public static String input;
    public static int calls;
    public static RuntimeException failure;
    public static CharSequence makeSafeForPresentation(String label, int length, float width,
            int flags) {
        calls++;
        input = label;
        if (length != 1000 || width != 1000f || flags != (1 | 4))
            throw new AssertionError("PackageItemInfo.loadSafeLabel parameters");
        if (failure != null) throw failure;
        return new StringBuilder("original result…");
    }
}
