package android.provider;
public class Settings {
    public static class Global {
        public static Integer setting;
        public static int getInt(android.content.ContentResolver resolver, String name, int def) {
            if (!name.equals("user_switcher_enabled")) throw new AssertionError(name);
            return setting == null ? def : setting;
        }
    }
}
