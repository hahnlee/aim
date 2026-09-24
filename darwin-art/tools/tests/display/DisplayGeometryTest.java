package dev.darwinart.runtime.display;

import android.app.WindowConfiguration;
import android.content.res.Configuration;
import android.graphics.Rect;

public final class DisplayGeometryTest {
    public static void main(String[] args) {
        Configuration source = new Configuration();
        source.screenLayout = Configuration.SCREENLAYOUT_SIZE_MASK
                | Configuration.SCREENLAYOUT_LONG_NO;

        DisplayGeometry initial = DisplayGeometry.initial();
        assert initial.widthPixels == BuiltInDisplayConfiguration.WIDTH_PIXELS;
        assert initial.heightPixels == BuiltInDisplayConfiguration.HEIGHT_PIXELS;
        Configuration portrait = initial.configuration(source);
        assert portrait.densityDpi == BuiltInDisplayConfiguration.DENSITY_DPI;
        assert portrait.screenWidthDp == BuiltInDisplayConfiguration.BASE_WIDTH_DP;
        assert portrait.screenHeightDp == BuiltInDisplayConfiguration.BASE_HEIGHT_DP;
        assert portrait.smallestScreenWidthDp == BuiltInDisplayConfiguration.BASE_WIDTH_DP;
        assert portrait.orientation == Configuration.ORIENTATION_PORTRAIT;
        assert (portrait.screenLayout & Configuration.SCREENLAYOUT_SIZE_MASK)
                == Configuration.SCREENLAYOUT_SIZE_NORMAL;
        assert (portrait.screenLayout & Configuration.SCREENLAYOUT_LONG_MASK)
                == Configuration.SCREENLAYOUT_LONG_YES;
        assert source.screenLayout == (Configuration.SCREENLAYOUT_SIZE_MASK
                | Configuration.SCREENLAYOUT_LONG_NO);
        assert sameBounds(portrait.windowConfiguration,
                new Rect(0, 0, initial.widthPixels, initial.heightPixels));

        // Landscape task at 2x: qualifiers in DIP, window bounds in pixels.
        DisplayGeometry landscape = new DisplayGeometry(3, 1280, 720, 320);
        Configuration wide = landscape.configuration(source);
        assert wide.screenWidthDp == 640 && wide.screenHeightDp == 360;
        assert wide.smallestScreenWidthDp == 360;
        assert wide.orientation == Configuration.ORIENTATION_LANDSCAPE;
        assert sameBounds(wide.windowConfiguration, new Rect(0, 0, 1280, 720));
        Configuration override = landscape.overrideConfiguration();
        assert override.screenWidthDp == 640 && override.densityDpi == 320;
        assert sameBounds(override.windowConfiguration, new Rect(0, 0, 1280, 720));

        // A large resized task gets a larger screen-size bucket.
        Configuration large = new DisplayGeometry(4, 2000, 1400, 320).configuration(source);
        assert (large.screenLayout & Configuration.SCREENLAYOUT_SIZE_MASK)
                == Configuration.SCREENLAYOUT_SIZE_LARGE;

        DisplayGeometry next = landscape.withExtent(4, 1400, 800);
        assert next.revision == 4 && next.densityDpi == 320 && next.sameExtent(1400, 800);
        boolean rejected = false;
        try {
            landscape.withExtent(3, 1, 1);
        } catch (IllegalArgumentException expected) {
            rejected = true;
        }
        assert rejected;
        System.out.println("DisplayGeometry PASS");
    }

    private static boolean sameBounds(WindowConfiguration window, Rect expected) {
        return sameRect(window.getBounds(), expected) && sameRect(window.getAppBounds(), expected)
                && sameRect(window.getMaxBounds(), expected);
    }

    private static boolean sameRect(Rect actual, Rect expected) {
        return actual.left == expected.left && actual.top == expected.top
                && actual.right == expected.right && actual.bottom == expected.bottom;
    }
}
