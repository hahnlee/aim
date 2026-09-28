package dev.aim.probe;

import java.io.File;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.LinkedHashMap;
import java.util.Map;

/** Exercises Android's real managed SystemFonts -> Minikin bootstrap for this isolated probe. */
final class FontBootstrap {
    private FontBootstrap() {}

    static void install() {
        String fontsXml = optionalEnvironment("AIM_HOST_FONTS_XML",
                "AIM_TEST_FONTS_XML");
        String roboto = optionalEnvironment("AIM_HOST_FONT",
                "AIM_TEST_FONT");
        try {
            Class<?> systemFonts = Class.forName("android.graphics.fonts.SystemFonts");
            Class<?> fontConfig = Class.forName("android.text.FontConfig");

            Map<String, File> updatedFonts = new LinkedHashMap<>();
            updatedFonts.put("Roboto-Regular", new File(roboto));
            // The AOSP Calculator display style requests sans-serif-light.
            // The Darwin fixture intentionally ships one pinned face, so map
            // Android's stock family names to that face instead of allowing
            // Typeface.create(null, style) during EditText construction.
            File regular = new File(roboto);
            updatedFonts.put("sans-serif", regular);
            updatedFonts.put("sans-serif-thin", regular);
            updatedFonts.put("sans-serif-light", regular);
            updatedFonts.put("sans-serif-medium", regular);
            updatedFonts.put("sans-serif-black", regular);

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
            java.lang.reflect.Field defaultField = typeface.getField("DEFAULT");
            java.lang.reflect.Field defaultBoldField = typeface.getField("DEFAULT_BOLD");
            java.lang.reflect.Field defaultTypefaceField =
                    typeface.getDeclaredField("sDefaultTypeface");
            java.lang.reflect.Field defaultsField =
                    typeface.getDeclaredField("sDefaults");
            defaultField.setAccessible(true);
            defaultBoldField.setAccessible(true);
            defaultTypefaceField.setAccessible(true);
            defaultsField.setAccessible(true);

            // setSystemFontMap derives styled defaults from sDefaultTypeface.
            // Zygote normally assigns that field while constructing this same
            // map; the detached startup must establish the seed before calling
            // the public replacement hook, not after it.
            Map<?, ?> builtTypefaces = (Map<?, ?>) typefaces;
            Object seedTypeface = builtTypefaces.get("sans-serif");
            if (seedTypeface == null) {
                for (Object candidate : builtTypefaces.values()) {
                    if (candidate != null) {
                        seedTypeface = candidate;
                        break;
                    }
                }
            }
            // A detached host can encounter families whose font files are not
            // present in the minimal system image. Android's zygote drops
            // those entries before publishing the map; passing null values to
            // setSystemFontMap instead reaches Typeface.create(null, style).
            // Preserve the family keys but substitute the verified seed face,
            // keeping the framework contract total without changing the APK.
            Map<Object, Object> completeTypefaces = new LinkedHashMap<>();
            for (Map.Entry<?, ?> entry : builtTypefaces.entrySet()) {
                completeTypefaces.put(entry.getKey(),
                        entry.getValue() == null ? seedTypeface : entry.getValue());
            }
            if (seedTypeface != null) {
                defaultField.set(null, seedTypeface);
                defaultBoldField.set(null, seedTypeface);
                defaultTypefaceField.set(null, seedTypeface);
                Object defaults = java.lang.reflect.Array.newInstance(typeface, 4);
                for (int index = 0; index < 4; index++) {
                    java.lang.reflect.Array.set(defaults, index, seedTypeface);
                }
                defaultsField.set(null, defaults);
            }
            Method installMap = typeface.getMethod("setSystemFontMap", Map.class);
            installMap.invoke(null, completeTypefaces);
            // Typeface's static defaults may have been touched by framework
            // startup before this detached launcher installs SystemFonts.  A
            // real zygote initializes these fields from the same map; restore
            // that invariant explicitly so TextView/EditText constructors do
            // not call Typeface.create(null, style).
            Method create = typeface.getMethod("create", String.class, int.class);
            Object regularTypeface = create.invoke(null, "sans-serif", 0);
            Object boldTypeface = create.invoke(null, "sans-serif", 1);
            if (regularTypeface != null) {
                defaultField.set(null, regularTypeface);
                defaultTypefaceField.set(null, regularTypeface);
            }
            if (boldTypeface != null) {
                defaultBoldField.set(null, boldTypeface);
            }
            if (regularTypeface != null && boldTypeface != null) {
                Object defaults = java.lang.reflect.Array.newInstance(typeface, 4);
                java.lang.reflect.Array.set(defaults, 0, regularTypeface);
                java.lang.reflect.Array.set(defaults, 1, boldTypeface);
                java.lang.reflect.Array.set(defaults, 2, regularTypeface);
                java.lang.reflect.Array.set(defaults, 3, regularTypeface);
                defaultsField.set(null, defaults);
            }
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

    private static String optionalEnvironment(String preferred, String fallback) {
        String value = System.getenv(preferred);
        return value == null || value.isEmpty() ? requireEnvironment(fallback) : value;
    }
}
