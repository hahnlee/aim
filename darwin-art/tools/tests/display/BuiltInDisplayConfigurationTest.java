package dev.darwinart.runtime.display;

import android.app.WindowConfiguration;
import android.content.res.Configuration;
import android.graphics.Rect;

public final class BuiltInDisplayConfigurationTest {
    public static void main(String[] args) {
        Configuration source = new Configuration();
        source.screenLayout = Configuration.SCREENLAYOUT_SIZE_MASK
                | Configuration.SCREENLAYOUT_LONG_NO;

        Configuration result = BuiltInDisplayConfiguration.configuration(source);

        assert result.densityDpi == BuiltInDisplayConfiguration.DENSITY_DPI;
        assert result.screenWidthDp == BuiltInDisplayConfiguration.BASE_WIDTH_DP;
        assert result.screenHeightDp == BuiltInDisplayConfiguration.BASE_HEIGHT_DP;
        assert result.smallestScreenWidthDp == BuiltInDisplayConfiguration.BASE_WIDTH_DP;
        assert result.orientation == Configuration.ORIENTATION_PORTRAIT;
        assert (result.screenLayout & Configuration.SCREENLAYOUT_SIZE_MASK)
                == Configuration.SCREENLAYOUT_SIZE_NORMAL;
        assert (result.screenLayout & Configuration.SCREENLAYOUT_LONG_MASK)
                == Configuration.SCREENLAYOUT_LONG_YES;
        assert source.screenLayout == (Configuration.SCREENLAYOUT_SIZE_MASK
                | Configuration.SCREENLAYOUT_LONG_NO);

        Rect expected = new Rect(0, 0, BuiltInDisplayConfiguration.WIDTH_PIXELS,
                BuiltInDisplayConfiguration.HEIGHT_PIXELS);
        WindowConfiguration window = result.windowConfiguration;
        assert sameRect(window.getBounds(), expected);
        assert sameRect(window.getAppBounds(), expected);
        assert sameRect(window.getMaxBounds(), expected);
    }

    private static boolean sameRect(Rect actual, Rect expected) {
        return actual.left == expected.left && actual.top == expected.top
                && actual.right == expected.right && actual.bottom == expected.bottom;
    }
}
