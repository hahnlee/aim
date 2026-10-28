package android.content.pm;
public final class PackageManager {
    public static final int GET_SHARED_LIBRARY_FILES=1024, MATCH_SYSTEM_ONLY=1048576;
    public static final String FEATURE_DEVICE_LOCK="android.software.device_lock";
    public static class NameNotFoundException extends Exception {}
    public ApplicationInfo getApplicationInfo(String name, int flags) {
        if (!name.equals("android") || flags != (GET_SHARED_LIBRARY_FILES|MATCH_SYSTEM_ONLY))
            throw new AssertionError("system application info");
        return null;
    }
    public boolean hasSystemFeature(String name) {
        if (!name.equals(FEATURE_DEVICE_LOCK)) throw new AssertionError(name);
        return dev.aim.server.SystemProcessHostTest.deviceLock;
    }
}
