package com.android.server.devicepolicy;
/** Observe the original-class boundary; the fixture value must not be hardcoded by the bridge. */
public class DevicePolicyCacheImpl {
    public static int calls;
    public static int user;
    public static RuntimeException failure;
    public int getContentProtectionPolicy(int userId) {
        calls++;
        user = userId;
        if (failure != null) throw failure;
        return 37;
    }
}
