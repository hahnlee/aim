// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.graphics;

public class Paint {
    public static final int ANTI_ALIAS_FLAG = 1;
    public static final int FILTER_BITMAP_FLAG = 2;
    public static final int DITHER_FLAG = 4;

    public Paint() { throw new RuntimeException("stub"); }
    public Paint(int flags) { throw new RuntimeException("stub"); }
    public void setColor(int color) { throw new RuntimeException("stub"); }
    public void setAlpha(int a) { throw new RuntimeException("stub"); }
    public void setStyle(Style style) { throw new RuntimeException("stub"); }

    public enum Style {
        FILL(0);

        Style(int nativeInt) { throw new RuntimeException("stub"); }
    }
}
