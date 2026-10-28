package com.android.server;
public final class LocalManagerRegistry {
    private static final java.util.Map<Class<?>,Object> values = new java.util.HashMap<>();
    public static <T> void addManager(Class<T> type, T value) { values.put(type, value); }
    public static <T> T getManager(Class<T> type) { return type.cast(values.get(type)); }
}
