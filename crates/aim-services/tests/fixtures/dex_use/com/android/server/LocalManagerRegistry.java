package com.android.server;
public final class LocalManagerRegistry {
 private static final java.util.Map<Class<?>,Object> managers=new java.util.HashMap<>();
 public static <T> T getManager(Class<T> type){return type.cast(managers.get(type));}
 public static <T> void addManager(Class<T> type,T manager){if(managers.putIfAbsent(type,manager)!=null)throw new AssertionError("duplicate manager");}
}
