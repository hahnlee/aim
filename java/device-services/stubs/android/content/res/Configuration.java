// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.res;

public final class Configuration {
    public static final int UI_MODE_NIGHT_MASK = 0x30;
    public static final int UI_MODE_NIGHT_YES = 0x20;

    public int uiMode;
    public int densityDpi;
    public final android.app.WindowConfiguration windowConfiguration = null;

    public Configuration() { throw new RuntimeException("stub"); }
    public void setToDefaults() { throw new RuntimeException("stub"); }
}
