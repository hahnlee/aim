package dev.darwinart.runtime.display;

import android.content.res.Configuration;
import android.graphics.Rect;
import java.lang.reflect.Field;
import java.lang.reflect.Method;

/** Immutable pixel/density contract for the built-in Android display. */
public final class BuiltInDisplayConfiguration {
    public static final int BASE_WIDTH_DP = 360;
    public static final int BASE_HEIGHT_DP = 640;
    public static final int SCALE =
            "2".equals(System.getenv("DARWIN_ART_WINDOW_SCALE")) ? 2 : 1;
    public static final int WIDTH_PIXELS = BASE_WIDTH_DP * SCALE;
    public static final int HEIGHT_PIXELS = BASE_HEIGHT_DP * SCALE;
    public static final int DENSITY_DPI = 160 * SCALE;

    private BuiltInDisplayConfiguration() {}

    /** Display-owned qualifiers merged with system locale and user settings. */
    public static Configuration configuration(Configuration system) {
        Configuration result = new Configuration(system);
        result.densityDpi = DENSITY_DPI;
        result.screenWidthDp = BASE_WIDTH_DP;
        result.screenHeightDp = BASE_HEIGHT_DP;
        result.smallestScreenWidthDp = Math.min(BASE_WIDTH_DP, BASE_HEIGHT_DP);
        result.orientation = WIDTH_PIXELS > HEIGHT_PIXELS
                ? Configuration.ORIENTATION_LANDSCAPE : Configuration.ORIENTATION_PORTRAIT;
        result.screenLayout = (result.screenLayout
                & ~(Configuration.SCREENLAYOUT_SIZE_MASK | Configuration.SCREENLAYOUT_LONG_MASK))
                | Configuration.SCREENLAYOUT_SIZE_NORMAL | Configuration.SCREENLAYOUT_LONG_YES;
        setWindowConfigurationBounds(result);
        return result;
    }

    /**
     * Keep Android 16's hidden window bounds in physical pixels while the qualifiers above stay
     * in logical DIP. WindowConfiguration is hidden from the public SDK, so this is the sole
     * compatibility boundary for that platform-owned field.
     */
    private static void setWindowConfigurationBounds(Configuration configuration) {
        try {
            Field field = Configuration.class.getField("windowConfiguration");
            Object windowConfiguration = field.get(configuration);
            if (windowConfiguration == null) {
                throw new IllegalStateException("Android WindowConfiguration is null");
            }
            Rect bounds = new Rect(0, 0, WIDTH_PIXELS, HEIGHT_PIXELS);
            Class<?> type = windowConfiguration.getClass();
            for (String setter : new String[] {"setBounds", "setAppBounds", "setMaxBounds"}) {
                Method method = type.getMethod(setter, Rect.class);
                method.invoke(windowConfiguration, bounds);
            }
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android WindowConfiguration ABI mismatch", error);
        }
    }
}
