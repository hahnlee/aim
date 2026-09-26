package dev.darwinart.runtime.display;

/**
 * The macOS display a desktop root is on, read from CoreGraphics by its
 * CGDirectDisplayID: its name, whether it is built in, and its physical
 * pixel density. The Android raster keeps 1:1 backing pixels, so the host
 * display's pixels per inch are Android's physical DPI.
 */
public final class HostDisplayFacts {
    public final String name;
    public final boolean builtIn;
    public final float xDpi;
    public final float yDpi;

    private HostDisplayFacts(String name, boolean builtIn, float xDpi, float yDpi) {
        this.name = name;
        this.builtIn = builtIn;
        this.xDpi = xDpi;
        this.yDpi = yDpi;
    }

    /** The display's facts, or null for an unknown or disconnected display. */
    public static HostDisplayFacts describe(int displayId) {
        if (displayId == 0) return null;
        float[] values = nativeDescribe(displayId);
        if (values == null || values.length != 3) return null;
        String name = nativeName(displayId);
        return new HostDisplayFacts(name == null || name.isEmpty() ? "Display" : name,
                values[0] != 0, values[1], values[2]);
    }

    // {builtIn, xDpi, yDpi}, or null.
    private static native float[] nativeDescribe(int displayId);
    private static native String nativeName(int displayId);
}
