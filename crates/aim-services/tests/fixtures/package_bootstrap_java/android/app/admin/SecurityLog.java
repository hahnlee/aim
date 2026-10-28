package android.app.admin;
public final class SecurityLog {
    public static boolean enabled;
    public static final int TAG_APP_PROCESS_START = 210005;
    public static final java.util.List<Object[]> events = new java.util.ArrayList<>();
    public static boolean isLoggingEnabled() { return enabled; }
    public static int writeEvent(int tag, Object... args) {
        if (tag != TAG_APP_PROCESS_START) throw new AssertionError(tag);
        events.add(args); return 0;
    }
}
