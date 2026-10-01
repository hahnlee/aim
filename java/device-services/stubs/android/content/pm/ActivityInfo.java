// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class ActivityInfo extends ComponentInfo {
    public static final int SCREEN_ORIENTATION_UNSPECIFIED = -1;

    public int screenOrientation;
    public WindowLayout windowLayout;
    public final int getThemeResource() { throw new RuntimeException("stub"); }

    public static final class WindowLayout {
        public final int minWidth;
        public final int minHeight;

        public WindowLayout(android.os.Parcel source) { throw new RuntimeException("stub"); }
    }
}
