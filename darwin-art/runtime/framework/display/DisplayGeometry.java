package dev.darwinart.runtime.display;

import android.content.res.Configuration;
import android.graphics.Rect;
import java.lang.reflect.Field;
import java.lang.reflect.Method;

/**
 * Immutable, revisioned geometry of one desktop task's logical display.
 *
 * <p>Pixels are Android pixels (the raster HWUI renders); density converts
 * them to DIP qualifiers. macOS points and backing scale are host provider
 * concerns and never appear here. The revision orders publications for one
 * task only; revisions from different tasks are unrelated.</p>
 */
public final class DisplayGeometry {
    public static final int DENSITY_DEFAULT = 160;
    private static final int MAXIMUM_DIMENSION = 16_384;

    public final long revision;
    public final int widthPixels;
    public final int heightPixels;
    public final int densityDpi;

    public DisplayGeometry(long revision, int widthPixels, int heightPixels, int densityDpi) {
        if (revision < 0 || !validDimension(widthPixels) || !validDimension(heightPixels)
                || densityDpi <= 0) {
            throw new IllegalArgumentException("invalid display geometry " + widthPixels + "x"
                    + heightPixels + "@" + densityDpi + " revision " + revision);
        }
        this.revision = revision;
        this.widthPixels = widthPixels;
        this.heightPixels = heightPixels;
        this.densityDpi = densityDpi;
    }

    /** The built-in portrait geometry used before any task policy is applied. */
    public static DisplayGeometry initial() {
        return new DisplayGeometry(0, BuiltInDisplayConfiguration.WIDTH_PIXELS,
                BuiltInDisplayConfiguration.HEIGHT_PIXELS,
                BuiltInDisplayConfiguration.DENSITY_DPI);
    }

    public static boolean validDimension(int value) {
        return value > 0 && value <= MAXIMUM_DIMENSION;
    }

    public DisplayGeometry withExtent(long nextRevision, int width, int height) {
        if (nextRevision <= revision) throw new IllegalArgumentException("revision must advance");
        return new DisplayGeometry(nextRevision, width, height, densityDpi);
    }

    public boolean sameExtent(int width, int height) {
        return widthPixels == width && heightPixels == height;
    }

    public boolean landscape() {
        return widthPixels > heightPixels;
    }

    public int widthDp() {
        return widthPixels * DENSITY_DEFAULT / densityDpi;
    }

    public int heightDp() {
        return heightPixels * DENSITY_DEFAULT / densityDpi;
    }

    public Rect bounds() {
        return new Rect(0, 0, widthPixels, heightPixels);
    }

    /** Process-global configuration: system locale/user settings plus display qualifiers. */
    public Configuration configuration(Configuration system) {
        Configuration result = new Configuration(system);
        result.densityDpi = densityDpi;
        result.screenWidthDp = widthDp();
        result.screenHeightDp = heightDp();
        result.smallestScreenWidthDp = Math.min(widthDp(), heightDp());
        result.orientation = landscape()
                ? Configuration.ORIENTATION_LANDSCAPE : Configuration.ORIENTATION_PORTRAIT;
        result.screenLayout = (result.screenLayout
                & ~(Configuration.SCREENLAYOUT_SIZE_MASK | Configuration.SCREENLAYOUT_LONG_MASK))
                | screenLayoutSize() | screenLayoutLong();
        setWindowConfigurationBounds(result, bounds());
        return result;
    }

    /**
     * Task-level override carried by Activity transactions. Only the fields this
     * task owns are defined; undefined fields inherit the process configuration.
     */
    public Configuration overrideConfiguration() {
        Configuration result = new Configuration(); // All fields start undefined.
        result.densityDpi = densityDpi;
        result.screenWidthDp = widthDp();
        result.screenHeightDp = heightDp();
        result.smallestScreenWidthDp = Math.min(widthDp(), heightDp());
        result.orientation = landscape()
                ? Configuration.ORIENTATION_LANDSCAPE : Configuration.ORIENTATION_PORTRAIT;
        setWindowConfigurationBounds(result, bounds());
        return result;
    }

    // Mirrors Configuration.reduceScreenLayout's size buckets for the task extent.
    private int screenLayoutSize() {
        int longDp = Math.max(widthDp(), heightDp());
        int shortDp = Math.min(widthDp(), heightDp());
        if (longDp >= 960 && shortDp >= 720) return Configuration.SCREENLAYOUT_SIZE_XLARGE;
        if (longDp >= 640 && shortDp >= 480) return Configuration.SCREENLAYOUT_SIZE_LARGE;
        if (longDp >= 470 && shortDp >= 320) return Configuration.SCREENLAYOUT_SIZE_NORMAL;
        return Configuration.SCREENLAYOUT_SIZE_SMALL;
    }

    private int screenLayoutLong() {
        int longDp = Math.max(widthDp(), heightDp());
        int shortDp = Math.min(widthDp(), heightDp());
        return (longDp * 3) / 5 >= shortDp - 1
                ? Configuration.SCREENLAYOUT_LONG_YES : Configuration.SCREENLAYOUT_LONG_NO;
    }

    /**
     * Android 16's hidden window bounds are physical pixels while the qualifiers
     * above are DIP. WindowConfiguration is hidden from the public SDK, so this
     * is the sole compatibility boundary for that platform-owned field.
     */
    static void setWindowConfigurationBounds(Configuration configuration, Rect bounds) {
        try {
            Field field = Configuration.class.getField("windowConfiguration");
            Object windowConfiguration = field.get(configuration);
            if (windowConfiguration == null) {
                throw new IllegalStateException("Android WindowConfiguration is null");
            }
            Class<?> type = windowConfiguration.getClass();
            for (String setter : new String[] {"setBounds", "setAppBounds", "setMaxBounds"}) {
                Method method = type.getMethod(setter, Rect.class);
                method.invoke(windowConfiguration, bounds);
            }
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android WindowConfiguration ABI mismatch", error);
        }
    }

    @Override
    public String toString() {
        return "DisplayGeometry{r" + revision + " " + widthPixels + "x" + heightPixels
                + "px " + densityDpi + "dpi}";
    }
}
