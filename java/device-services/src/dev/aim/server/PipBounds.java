package dev.aim.server;

import android.app.ActivityManager.RunningTaskInfo;
import android.app.PictureInPictureParams;
import android.content.pm.ActivityInfo;
import android.content.res.Resources;
import android.graphics.Rect;

/**
 * Where a picture-in-picture task goes (docs/task-organizer.md,
 * "Picture-in-picture"): WMShell's default for phones, ported from its
 * PipBoundsAlgorithm and PhoneSizeSpecSource (Apache-2.0) with the image's
 * SystemUI values. Its size follows the params' aspect ratio, between the
 * minimum and maximum the platform allows (or is the activity's own minimal
 * size, from its manifest layout), and it sits in the display's
 * bottom right corner, 16 dp from the edges. The display has no system
 * bars to keep clear of: the lightweight shell has none.
 */
final class PipBounds {
    /** WMShell's config_pictureInPictureDefaultAspectRatio. */
    private static final float DEFAULT_ASPECT_RATIO = 1.77778f;
    /** config_pipLargeScreenOptimizedAspectRatio. */
    private static final float OPTIMIZED_ASPECT_RATIO = 0.5625f;
    /** config_pipSquareDisplayThresholdForSystemPreferredSize. */
    private static final float SQUARE_THRESHOLD = 0.95f;
    /** config_pipSystemPreferred{Default,Minimum}SizePercent, and ForSquareDisplay. */
    private static final float DEFAULT_PERCENT = 0.6f;
    private static final float MINIMUM_PERCENT = 0.5f;
    private static final float DEFAULT_PERCENT_SQUARE = 0.5f;
    private static final float MINIMUM_PERCENT_SQUARE = 0.4f;
    /** default_minimal_size_pip_resizable_task, in dp. */
    private static final int MIN_SIZE_DP = 108;
    /** overridable_minimal_size_pip_resizable_task, in dp. */
    private static final int OVERRIDABLE_MIN_SIZE_DP = 48;
    /** config_defaultPictureInPictureScreenEdgeInsets, in dp. */
    private static final int EDGE_DP = 16;

    private PipBounds() {}

    /** The aspect ratio `task`'s params ask for, if the platform allows it. */
    static float aspectRatio(RunningTaskInfo task, Resources res) {
        PictureInPictureParams params = task.pictureInPictureParams;
        if (params == null || !params.hasSetAspectRatio()) {
            return DEFAULT_ASPECT_RATIO;
        }
        float ratio = params.getAspectRatioFloat();
        float min = res.getFloat(com.android.internal.R.dimen.config_pictureInPictureMinAspectRatio);
        float max = res.getFloat(com.android.internal.R.dimen.config_pictureInPictureMaxAspectRatio);
        return Float.compare(min, ratio) <= 0 && Float.compare(ratio, max) <= 0
                ? ratio : DEFAULT_ASPECT_RATIO;
    }

    /**
     * The bounds of `task` in PiP at `aspectRatio`: its default size, with
     * its bottom right corner at `corner`'s (the display's own when null),
     * within the display.
     */
    static Rect bounds(RunningTaskInfo task, float aspectRatio, Rect corner) {
        Rect display = task.configuration.windowConfiguration.getMaxBounds();
        float density = task.configuration.densityDpi / 160f;
        int edge = Math.round(EDGE_DP * density);
        Rect inset = new Rect(display.left + edge, display.top + edge,
                display.right - edge, display.bottom - edge);
        ActivityInfo.WindowLayout layout =
                task.topActivityInfo != null ? task.topActivityInfo.windowLayout : null;
        int[] size = layout != null && layout.minWidth > 0 && layout.minHeight > 0
                ? overrideSize(layout, aspectRatio, density)
                : defaultSize(display, inset, aspectRatio, density);
        int width = size[0];
        int height = size[1];
        int right = corner != null ? Math.min(corner.right, inset.right) : inset.right;
        int bottom = corner != null ? Math.min(corner.bottom, inset.bottom) : inset.bottom;
        right = Math.max(right, inset.left + width);
        bottom = Math.max(bottom, inset.top + height);
        return new Rect(right - width, bottom - height, right, bottom);
    }

    /**
     * The size of an activity with a minimal size in its manifest layout:
     * that size, at least the overridable minimum, fitted to `aspectRatio`
     * (WMShell's override minimum size, which is then its default size).
     */
    private static int[] overrideSize(ActivityInfo.WindowLayout layout, float aspectRatio,
            float density) {
        int edge = Math.round(OVERRIDABLE_MIN_SIZE_DP * density);
        int width = Math.max(layout.minWidth, edge);
        int height = Math.max(layout.minHeight, edge);
        return width / (float) height > aspectRatio
                ? new int[] {width, (int) (width / aspectRatio)}
                : new int[] {(int) (height * aspectRatio), height};
    }

    /** The default size: a share of the largest PiP of `aspectRatio`. */
    private static int[] defaultSize(Rect display, Rect inset, float aspectRatio,
            float density) {
        boolean square = Math.min(display.width(), display.height())
                / (float) Math.max(display.width(), display.height()) > SQUARE_THRESHOLD;
        int shorter = Math.min(inset.width(), inset.height());
        int[] max = maxSize(aspectRatio, shorter);
        int minWidth = Math.round(max[0] * (square ? MINIMUM_PERCENT_SQUARE : MINIMUM_PERCENT));
        int minSize = Math.round(MIN_SIZE_DP * density);
        if (aspectRatio > 1f) {
            int minHeight = Math.max(
                    Math.round(max[1] * (square ? MINIMUM_PERCENT_SQUARE : MINIMUM_PERCENT)),
                    minSize);
            minWidth = Math.round(minHeight * aspectRatio);
        } else {
            minWidth = Math.max(minWidth, minSize);
        }
        int width = Math.max(
                Math.round(max[0] * (square ? DEFAULT_PERCENT_SQUARE : DEFAULT_PERCENT)), minWidth);
        return new int[] {width, Math.round(width / aspectRatio)};
    }

    /**
     * The largest PiP of `aspectRatio` with `shorter` the inset display's
     * shorter side: 16:9 takes all of it, and closer to square less.
     */
    private static int[] maxSize(float aspectRatio, int shorter) {
        int width;
        int height;
        if (aspectRatio >= OPTIMIZED_ASPECT_RATIO && aspectRatio <= 1 / OPTIMIZED_ASPECT_RATIO) {
            width = Math.min(Math.round(OPTIMIZED_ASPECT_RATIO * shorter
                    + shorter * (aspectRatio - OPTIMIZED_ASPECT_RATIO) / (1 + aspectRatio)),
                    shorter);
            height = Math.round(width / aspectRatio);
        } else if (aspectRatio > 1f) {
            width = shorter;
            height = Math.round(width / aspectRatio);
        } else {
            height = shorter;
            width = Math.round(height * aspectRatio);
        }
        return new int[] {width, height};
    }
}
