package android.content.res;

import android.app.WindowConfiguration;

public class Configuration {
    public static final int ORIENTATION_PORTRAIT = 1;
    public static final int ORIENTATION_LANDSCAPE = 2;
    public static final int SCREENLAYOUT_SIZE_MASK = 0x0f;
    public static final int SCREENLAYOUT_SIZE_SMALL = 0x01;
    public static final int SCREENLAYOUT_SIZE_NORMAL = 0x02;
    public static final int SCREENLAYOUT_SIZE_LARGE = 0x03;
    public static final int SCREENLAYOUT_SIZE_XLARGE = 0x04;
    public static final int SCREENLAYOUT_LONG_MASK = 0x30;
    public static final int SCREENLAYOUT_LONG_NO = 0x10;
    public static final int SCREENLAYOUT_LONG_YES = 0x20;
    public int densityDpi, screenWidthDp, screenHeightDp, smallestScreenWidthDp;
    public int orientation, screenLayout;
    public final WindowConfiguration windowConfiguration = new WindowConfiguration();
    public Configuration() {}
    public Configuration(Configuration other) {}
    public void setConfiguration(Configuration global, Configuration override) {}
}
