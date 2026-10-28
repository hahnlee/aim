package android.app;
public final class ActivityManager {
    public static boolean available = true;
    public static IActivityManager getService() { return available ? new IActivityManager() {} : null; }
}
