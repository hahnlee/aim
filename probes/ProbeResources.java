package dev.aim.probe;

import android.content.res.AssetManager;
import android.content.res.Configuration;
import android.content.res.Resources;
import android.content.res.XmlResourceParser;
import android.util.DisplayMetrics;
import android.util.TypedValue;
import java.util.Locale;

@SuppressWarnings("deprecation")
public final class ProbeResources extends Resources {
    private static final Configuration CONFIGURATION = new Configuration();
    private static final DisplayMetrics DISPLAY_METRICS = new DisplayMetrics();

    static {
        CONFIGURATION.setToDefaults();
        CONFIGURATION.setLocale(Locale.US);
        DISPLAY_METRICS.setToDefaults();
    }

    private final boolean frameworkResources;

    public static synchronized void configureDisplayScale(int scale) {
        if (scale != 1 && scale != 2) {
            throw new IllegalArgumentException("display scale must be 1 or 2");
        }
        configureDisplaySize(360 * scale, 640 * scale, scale);
    }

    /** Updates the shared Android display contract after a host rotation. */
    public static synchronized void configureDisplaySize(int widthPixels, int heightPixels) {
        if (widthPixels <= 0 || heightPixels <= 0) {
            throw new IllegalArgumentException("display dimensions must be positive");
        }
        int scale = DISPLAY_METRICS.densityDpi >= DisplayMetrics.DENSITY_DEFAULT * 2
                ? 2 : 1;
        configureDisplaySize(widthPixels, heightPixels, scale);
    }

    private static void configureDisplaySize(int widthPixels, int heightPixels, int scale) {
        int densityDpi = DisplayMetrics.DENSITY_DEFAULT * scale;
        int widthDp = Math.max(1, widthPixels / scale);
        int heightDp = Math.max(1, heightPixels / scale);
        CONFIGURATION.setToDefaults();
        CONFIGURATION.setLocale(Locale.US);
        CONFIGURATION.densityDpi = densityDpi;
        // The detached host has no WindowManager to populate a configuration.
        // Android resource aliases (for example Calculator's generic layout
        // alias to its port/land variants) require a concrete orientation and
        // screen bucket, otherwise Resources.getLayout() cannot select an
        // entry even though the APK's resource table is installed.
        CONFIGURATION.orientation = widthPixels > heightPixels
                ? Configuration.ORIENTATION_LANDSCAPE
                : widthPixels < heightPixels
                        ? Configuration.ORIENTATION_PORTRAIT
                        : Configuration.ORIENTATION_SQUARE;
        CONFIGURATION.screenWidthDp = widthDp;
        CONFIGURATION.screenHeightDp = heightDp;
        CONFIGURATION.smallestScreenWidthDp = Math.min(widthDp, heightDp);
        try {
            Object windowConfiguration = Configuration.class
                    .getField("windowConfiguration").get(CONFIGURATION);
            android.graphics.Rect bounds =
                    new android.graphics.Rect(0, 0, widthPixels, heightPixels);
            for (String setter : new String[] {"setBounds", "setAppBounds",
                                                "setMaxBounds"}) {
                windowConfiguration.getClass().getMethod(setter, android.graphics.Rect.class)
                        .invoke(windowConfiguration, bounds);
            }
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android window bounds unavailable", error);
        }
        // The macOS host is a touch-capable Android display paired with a
        // permanently connected physical keyboard.  Android publishes this
        // through Configuration in addition to InputManager device records;
        // applications use these qualifiers to enable their hardware-key
        // navigation paths and to avoid assuming a software IME is required.
        CONFIGURATION.touchscreen = Configuration.TOUCHSCREEN_FINGER;
        CONFIGURATION.keyboard = Configuration.KEYBOARD_QWERTY;
        CONFIGURATION.keyboardHidden = Configuration.KEYBOARDHIDDEN_NO;
        CONFIGURATION.hardKeyboardHidden = Configuration.HARDKEYBOARDHIDDEN_NO;
        DISPLAY_METRICS.setToDefaults();
        DISPLAY_METRICS.widthPixels = widthPixels;
        DISPLAY_METRICS.heightPixels = heightPixels;
        DISPLAY_METRICS.density = scale;
        DISPLAY_METRICS.densityDpi = densityDpi;
        DISPLAY_METRICS.scaledDensity = scale;
        DISPLAY_METRICS.xdpi = densityDpi;
        DISPLAY_METRICS.ydpi = densityDpi;
    }

    public ProbeResources(AssetManager assets, boolean frameworkResources) {
        super(assets, DISPLAY_METRICS, CONFIGURATION);
        this.frameworkResources = frameworkResources;
    }

    @Override
    public int getIdentifier(String name, String defType, String defPackage) {
        int identifier = super.getIdentifier(name, defType, defPackage);
        String appPackage = System.getenv("AIM_APK_APP_PACKAGE");
        // Android's ResourcesImpl resolves an unqualified application name in
        // the caller's package even when a library supplied a framework
        // package as its default. The detached host has one AssetManager for
        // framework + APK, so repeat the lookup against the installed app
        // package before reporting a miss.
        if (identifier == 0 && appPackage != null && !appPackage.isEmpty()
                && !appPackage.equals(defPackage)) {
            identifier = super.getIdentifier(name, defType, appPackage);
        }
        if (System.getenv("AIM_DEBUG_RESOURCES") != null) {
            System.err.println("DARWIN resources identifier name=" + name
                    + " type=" + defType + " package=" + defPackage
                    + " app=" + appPackage + " result=0x"
                    + Integer.toHexString(identifier));
        }
        return identifier;
    }

    @Override
    public String getResourcePackageName(int id) throws NotFoundException {
        String resolved;
        if ((id >>> 24) == 0x7f) {
            String appPackage = System.getenv("AIM_APK_APP_PACKAGE");
            if (appPackage != null && !appPackage.isEmpty()) {
                resolved = appPackage;
                if (System.getenv("AIM_DEBUG_RESOURCES") != null) {
                    System.err.println("DARWIN resources package id=0x"
                            + Integer.toHexString(id) + " result=" + resolved);
                }
                return resolved;
            }
        }
        resolved = super.getResourcePackageName(id);
        if (System.getenv("AIM_DEBUG_RESOURCES") != null) {
            System.err.println("DARWIN resources package id=0x"
                    + Integer.toHexString(id) + " result=" + resolved);
        }
        return resolved;
    }

    @Override
    public boolean getBoolean(int id) {
        try {
            return super.getBoolean(id);
        } catch (Resources.NotFoundException missing) {
            return false;
        }
    }

    @Override
    public int getDimensionPixelSize(int id) {
        try {
            return super.getDimensionPixelSize(id);
        } catch (Resources.NotFoundException missing) {
            return 8;
        }
    }

    @Override
    public int getColor(int id) {
        try {
            return super.getColor(id);
        } catch (Resources.NotFoundException missing) {
            return 0xff2563eb;
        }
    }

    @Override
    public int getColor(int id, Theme theme) {
        try {
            return super.getColor(id, theme);
        } catch (Resources.NotFoundException missing) {
            return 0xff2563eb;
        }
    }

    @Override
    public int getInteger(int id) {
        try {
            return super.getInteger(id);
        } catch (Resources.NotFoundException missing) {
            // Preserve the small standalone probe's historical fallback for
            // IDs that are intentionally absent from its synthetic table.
            return 2;
        }
    }

    @Override
    public XmlResourceParser getAnimation(int id) {
        return frameworkResources ? super.getAnimation(id) : new ProbeXmlResourceParser();
    }

    @Override
    public void getValue(int id, TypedValue outValue, boolean resolveRefs) {
        try {
            super.getValue(id, outValue, resolveRefs);
            return;
        } catch (Resources.NotFoundException missing) {
            // Never disguise an installed APK resource as the probe's
            // synthetic float. App code commonly catches NotFoundException
            // and applies its own default; returning TYPE_FLOAT here breaks
            // type-sensitive values such as Chromium's window fractions.
            if (frameworkResources && (id >>> 24) == 0x7f) throw missing;
            // Synthetic probe-only resources have no backing table.
        }
        outValue.type = TypedValue.TYPE_FLOAT;
        outValue.data = Float.floatToRawIntBits(2.0f);
    }

    @Override
    public Configuration getConfiguration() {
        return CONFIGURATION;
    }

    @Override
    public DisplayMetrics getDisplayMetrics() {
        return DISPLAY_METRICS;
    }
}
