package android.provider;
public final class Settings {
    public static final class Global {
        public static final String DEVICE_DEMO_MODE = "device_demo_mode";
        public static int getInt(android.content.ContentResolver resolver, String name, int fallback) { return fallback; }
    }
}
