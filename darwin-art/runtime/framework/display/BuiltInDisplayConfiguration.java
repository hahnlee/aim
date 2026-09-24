package dev.darwinart.runtime.display;

/** Initial built-in display extent and density before task geometry is applied. */
public final class BuiltInDisplayConfiguration {
    public static final int BASE_WIDTH_DP = 360;
    public static final int BASE_HEIGHT_DP = 640;
    public static final int SCALE =
            "2".equals(System.getenv("DARWIN_ART_WINDOW_SCALE")) ? 2 : 1;
    public static final int WIDTH_PIXELS = BASE_WIDTH_DP * SCALE;
    public static final int HEIGHT_PIXELS = BASE_HEIGHT_DP * SCALE;
    public static final int DENSITY_DPI = 160 * SCALE;

    private BuiltInDisplayConfiguration() {}
}
