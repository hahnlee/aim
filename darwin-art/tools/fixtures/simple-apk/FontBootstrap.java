package dev.darwinart.simple;

import java.io.File;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.LinkedHashMap;
import java.util.Map;

/** Installs the real Android SystemFonts/Minikin typeface map for the APK. */
final class FontBootstrap {
    private FontBootstrap() {}

    static void install() {
        String fontsXml = requireEnvironment("DARWIN_ART_TEST_FONTS_XML");
        String roboto = requireEnvironment("DARWIN_ART_TEST_FONT");
        try {
            Class<?> systemFonts = Class.forName("android.graphics.fonts.SystemFonts");
            Class<?> fontConfig = Class.forName("android.text.FontConfig");
            Map<String, File> updatedFonts = new LinkedHashMap<>();
            updatedFonts.put("Roboto-Regular", new File(roboto));
            Method readConfig = systemFonts.getDeclaredMethod(
                    "getSystemFontConfigInternal",
                    String.class,
                    String.class,
                    String.class,
                    String.class,
                    Map.class,
                    long.class,
                    int.class);
            readConfig.setAccessible(true);
            String fontDirectory = new File(roboto).getParent() + File.separator;
            Object config = readConfig.invoke(
                    null, fontsXml, fontDirectory, null, null, updatedFonts, 0L, 0);
            Method buildFallback = systemFonts.getMethod("buildSystemFallback", fontConfig);
            Object fallback = buildFallback.invoke(null, config);
            Method buildTypefaces = systemFonts.getMethod(
                    "buildSystemTypefaces", fontConfig, Map.class);
            Object typefaces = buildTypefaces.invoke(null, config, fallback);
            Class<?> typeface = Class.forName("android.graphics.Typeface");
            Method installMap = typeface.getMethod("setSystemFontMap", Map.class);
            installMap.invoke(null, typefaces);
        } catch (InvocationTargetException error) {
            Throwable cause = error.getCause();
            if (cause instanceof RuntimeException) {
                throw (RuntimeException) cause;
            }
            if (cause instanceof Error) {
                throw (Error) cause;
            }
            throw new IllegalStateException("Android system font bootstrap failed", cause);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not initialize Android system fonts", error);
        }
    }

    private static String requireEnvironment(String name) {
        String value = System.getenv(name);
        if (value == null || value.isEmpty()) {
            throw new IllegalStateException(name + " is required");
        }
        return value;
    }
}
