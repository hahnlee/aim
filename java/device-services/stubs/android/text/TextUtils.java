// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.text;

public class TextUtils {
    private TextUtils() {}
    public static final int SAFE_STRING_FLAG_TRIM = 1;
    public static final int SAFE_STRING_FLAG_FIRST_LINE = 4;
    public static CharSequence makeSafeForPresentation(String unclean,
            int maxCharactersToConsider, float ellipsizeDip, int flags) {
        throw new RuntimeException("stub");
    }
}
