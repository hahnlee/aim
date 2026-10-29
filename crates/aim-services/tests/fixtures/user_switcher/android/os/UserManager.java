package android.os;
public class UserManager {
    public static boolean multiple = true, demo;
    public static boolean supportsMultipleUsers() { return multiple; }
    public static boolean isDeviceInDemoMode(android.content.Context c) { return demo; }
}
