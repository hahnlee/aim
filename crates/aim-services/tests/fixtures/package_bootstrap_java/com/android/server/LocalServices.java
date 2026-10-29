package com.android.server;
public final class LocalServices {
    private static final java.util.Map<Class<?>, Object> services = new java.util.HashMap<>();
    public static <T> T getService(Class<T> type) { return type.cast(services.get(type)); }
    public static int packageRegistrations;
    public static <T> void addService(Class<T> type, T service) {
        if (type == android.content.pm.PackageManagerInternal.class) packageRegistrations++;
        services.put(type, service);
    }
    public static <T> void removeServiceForTest(Class<T> type) { services.remove(type); }
}
