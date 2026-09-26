package dev.darwinart.runtime.display;

import android.os.Parcelable;
import android.view.Display;
import java.lang.reflect.Array;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;

/** Android-owned DisplayInfo projection of one task's logical display 0. */
final class DefaultDisplayRegistry {
    private static final int MODE_ID = 1;
    private static final int DISPLAY_TYPE_INTERNAL = 1;
    private static final int DISPLAY_TYPE_EXTERNAL = 2;

    int[] getDisplayIds(boolean includeDisabled) {
        return new int[] {Display.DEFAULT_DISPLAY};
    }

    /**
     * {@code host} is the macOS display the task's root is on, when known: it
     * names the display and gives its type and physical pixel density.
     */
    Parcelable getDisplayInfo(int displayId, DisplayGeometry geometry, HostDisplayFacts host) {
        if (displayId != Display.DEFAULT_DISPLAY) return null;
        try {
            Class<?> infoClass = Class.forName("android.view.DisplayInfo");
            Object info = infoClass.getDeclaredConstructor().newInstance();
            set(infoClass, info, "displayId", Display.DEFAULT_DISPLAY);
            set(infoClass, info, "layerStack", Display.DEFAULT_DISPLAY);
            set(infoClass, info, "name", host == null ? "Built-in Display" : host.name);
            set(infoClass, info, "uniqueId", "local:darwin-art:0");
            set(infoClass, info, "type",
                    host == null || host.builtIn ? DISPLAY_TYPE_INTERNAL : DISPLAY_TYPE_EXTERNAL);
            set(infoClass, info, "state", Display.STATE_ON);
            set(infoClass, info, "committedState", Display.STATE_ON);
            // A desktop task's display has no rotation: its natural extent is
            // the current task extent, so nominal sizes follow the same bounds.
            int width = geometry.widthPixels;
            int height = geometry.heightPixels;
            int densityDpi = geometry.densityDpi;
            set(infoClass, info, "logicalWidth", width);
            set(infoClass, info, "logicalHeight", height);
            set(infoClass, info, "appWidth", width);
            set(infoClass, info, "appHeight", height);
            set(infoClass, info, "smallestNominalAppWidth", Math.min(width, height));
            set(infoClass, info, "smallestNominalAppHeight", Math.min(width, height));
            set(infoClass, info, "largestNominalAppWidth", Math.max(width, height));
            set(infoClass, info, "largestNominalAppHeight", Math.max(width, height));
            set(infoClass, info, "logicalDensityDpi", densityDpi);
            set(infoClass, info, "physicalXDpi", host == null ? (float) densityDpi : host.xDpi);
            set(infoClass, info, "physicalYDpi", host == null ? (float) densityDpi : host.yDpi);

            Class<?> modeClass = Class.forName("android.view.Display$Mode");
            Constructor<?> modeConstructor = modeClass.getDeclaredConstructor(
                    int.class, int.class, int.class, float.class);
            modeConstructor.setAccessible(true);
            // The root's display paces vsync (ProcessFrameIntervalNanos).
            float refreshRate = host == null ? 60.0f : host.refreshRate;
            Object mode = modeConstructor.newInstance(MODE_ID, width, height, refreshRate);
            Object modes = Array.newInstance(modeClass, 1);
            Array.set(modes, 0, mode);
            set(infoClass, info, "modeId", MODE_ID);
            set(infoClass, info, "defaultModeId", MODE_ID);
            set(infoClass, info, "supportedModes", modes);
            set(infoClass, info, "appsSupportedModes", modes);
            set(infoClass, info, "supportedRefreshRates", new float[] {refreshRate});
            set(infoClass, info, "refreshRateOverride", refreshRate);
            set(infoClass, info, "renderFrameRate", refreshRate);
            set(infoClass, info, "presentationDeadlineNanos",
                    (long) (1_000_000_000L / refreshRate));
            set(infoClass, info, "canHostTasks", true);
            return (Parcelable) info;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android DisplayInfo ABI mismatch", error);
        }
    }

    private static void set(Class<?> type, Object target, String name, Object value)
            throws ReflectiveOperationException {
        Field field = type.getDeclaredField(name);
        field.setAccessible(true);
        field.set(target, value);
    }
}
