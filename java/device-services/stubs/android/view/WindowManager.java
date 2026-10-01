// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.view;

public interface WindowManager {
    int TRANSIT_OPEN = 1;
    int TRANSIT_TO_FRONT = 3;
    int TRANSIT_CHANGE = 6;
    int TRANSIT_PIP = 10;

    class BadTokenException extends RuntimeException {
        public BadTokenException() { throw new RuntimeException("stub"); }
    }

    class LayoutParams extends ViewGroup.LayoutParams {
        public static final int TYPE_APPLICATION_STARTING = 3;
        public static final int FLAG_NOT_FOCUSABLE = 8;
        public static final int FLAG_NOT_TOUCHABLE = 16;
        public static final int FLAG_LAYOUT_IN_SCREEN = 256;
        public static final int FLAG_LAYOUT_INSET_DECOR = 65536;
        public static final int FLAG_ALT_FOCUSABLE_IM = 131072;
        public static final int FLAG_SHOW_WHEN_LOCKED = 524288;
        public static final int FLAG_HARDWARE_ACCELERATED = 16777216;
        public static final int FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS = -2147483648;
        public static final int SYSTEM_FLAG_SHOW_FOR_ALL_USERS = 16;
        public static final int LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS = 3;
        public int flags;
        public int privateFlags;
        public int format;
        public int layoutInDisplayCutoutMode;
        public android.os.IBinder token;
        public String packageName;
        public LayoutParams(int type) { super(0, 0); throw new RuntimeException("stub"); }
        public void setFitInsetsSides(int sides) { throw new RuntimeException("stub"); }
        public void setFitInsetsTypes(int types) { throw new RuntimeException("stub"); }
        public final void setTitle(CharSequence title) { throw new RuntimeException("stub"); }
    }
}
