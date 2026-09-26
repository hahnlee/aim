package android.provider;

public final class Settings {
    public static final class Global {
        public static final String ANIMATOR_DURATION_SCALE = "animator_duration_scale";

        public static float getFloat(android.content.ContentResolver resolver, String name,
                float fallback) {
            return fallback;
        }
    }
}
