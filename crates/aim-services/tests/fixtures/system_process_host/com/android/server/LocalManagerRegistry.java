package com.android.server;
public final class LocalManagerRegistry {
    private static Object helper;
    public static <T> void addManager(Class<T> kind, T value) {
        if (kind != com.android.server.role.RoleServicePlatformHelper.class || helper != null)
            throw new AssertionError(kind);
        helper = value;
        dev.aim.server.SystemProcessHostTest.events.add("roleHelper");
    }
    public static <T> T getManager(Class<T> kind) { return kind.cast(helper); }
}
