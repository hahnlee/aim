package android.content.res;

import android.app.WindowConfiguration;

public class Configuration {
    public static final int ORIENTATION_UNDEFINED = 0;
    public static final int ORIENTATION_PORTRAIT = 1;
    public static final int ORIENTATION_LANDSCAPE = 2;
    public static final int SCREENLAYOUT_SIZE_MASK = 0x0f;
    public static final int SCREENLAYOUT_SIZE_NORMAL = 0x02;
    public static final int SCREENLAYOUT_LONG_MASK = 0x30;
    public static final int SCREENLAYOUT_LONG_NO = 0x10;
    public static final int SCREENLAYOUT_LONG_YES = 0x20;

    public int densityDpi;
    public int screenWidthDp;
    public int screenHeightDp;
    public int smallestScreenWidthDp;
    public int orientation;
    public int screenLayout;
    public final WindowConfiguration windowConfiguration;

    public Configuration() {
        windowConfiguration = new WindowConfiguration();
    }

    public Configuration(Configuration other) {
        densityDpi = other.densityDpi;
        screenWidthDp = other.screenWidthDp;
        screenHeightDp = other.screenHeightDp;
        smallestScreenWidthDp = other.smallestScreenWidthDp;
        orientation = other.orientation;
        screenLayout = other.screenLayout;
        windowConfiguration = new WindowConfiguration(other.windowConfiguration);
    }
}
