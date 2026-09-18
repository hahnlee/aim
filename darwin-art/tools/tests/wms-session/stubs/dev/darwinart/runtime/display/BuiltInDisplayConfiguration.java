package dev.darwinart.runtime.display;

import android.content.res.Configuration;

public final class BuiltInDisplayConfiguration {
    public static final int WIDTH_PIXELS = 1920;
    public static final int HEIGHT_PIXELS = 1080;
    public static Configuration configuration(Configuration base) { return new Configuration(base); }
}
