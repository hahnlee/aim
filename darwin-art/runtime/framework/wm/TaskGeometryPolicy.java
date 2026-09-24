package dev.darwinart.runtime.wm;

/**
 * Pure ActivityTask policy for a desktop task's extent. The desktop window has
 * no sensor rotation: a fixed orientation request swaps the task extent, while
 * user and sensor orientations keep the size chosen by the window owner.
 */
final class TaskGeometryPolicy {
    static final int NONE = 0;
    static final int LANDSCAPE = 1;
    static final int PORTRAIT = 2;

    // android.content.pm.ActivityInfo.CONFIG_* bits used by relaunch policy.
    static final int CONFIG_SCREEN_SIZE = 0x0400;
    static final int CONFIG_SMALLEST_SCREEN_SIZE = 0x0800;
    static final int CONFIG_WINDOW_CONFIGURATION = 0x20000000;
    static final int CONFIG_ASSETS_PATHS = 0x80000000;
    private static final int HONEYCOMB_MR2 = 13;

    private TaskGeometryPolicy() {}

    /** Maps ActivityInfo.screenOrientation to the task shape it requires. */
    static int requiredShape(int screenOrientation) {
        switch (screenOrientation) {
            case 0:  // SCREEN_ORIENTATION_LANDSCAPE
            case 6:  // SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            case 8:  // SCREEN_ORIENTATION_REVERSE_LANDSCAPE
            case 11: // SCREEN_ORIENTATION_USER_LANDSCAPE
                return LANDSCAPE;
            case 1:  // SCREEN_ORIENTATION_PORTRAIT
            case 7:  // SCREEN_ORIENTATION_SENSOR_PORTRAIT
            case 9:  // SCREEN_ORIENTATION_REVERSE_PORTRAIT
            case 12: // SCREEN_ORIENTATION_USER_PORTRAIT
                return PORTRAIT;
            default:
                return NONE;
        }
    }

    /** Returns {width, height} satisfying {@code shape}, preserving the area. */
    static int[] extentFor(int width, int height, int shape) {
        if (shape == LANDSCAPE && height > width) return new int[] {height, width};
        if (shape == PORTRAIT && width > height) return new int[] {height, width};
        return new int[] {width, height};
    }

    /**
     * Configuration changes the Activity handles itself, as ActivityInfo's
     * getRealConfigChanged(): pre-HONEYCOMB_MR2 apps implicitly handle size.
     */
    static int handledChanges(int configChanges, int targetSdkVersion) {
        int handled = configChanges;
        if (targetSdkVersion < HONEYCOMB_MR2) {
            handled |= CONFIG_SCREEN_SIZE | CONFIG_SMALLEST_SCREEN_SIZE;
        }
        return handled;
    }

    /** ActivityRecord.getConfigurationChanges: window-configuration-only diffs never relaunch. */
    static int reportableChanges(int diff) {
        return diff & ~(CONFIG_WINDOW_CONFIGURATION | CONFIG_ASSETS_PATHS);
    }

    static boolean shouldRelaunch(int changes, int handled) {
        return (changes & ~handled) != 0;
    }

    /** Host points to Android pixels for the configured Android raster scale. */
    static int pixelsFromPoints(int points, int scale) {
        long pixels = (long) points * scale;
        return pixels > Integer.MAX_VALUE ? Integer.MAX_VALUE : (int) pixels;
    }

    static int pointsFromPixels(int pixels, int scale) {
        return (pixels + scale - 1) / scale;
    }
}
