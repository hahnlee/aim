package com.android.internal.util;
public interface Parcelling<T> {
    final class BuiltIn {public static final class ForStringSet implements Parcelling<java.util.Set<String>> {}}
    final class Cache {
        public static <T extends Parcelling<?>> T get(Class<T> type){return null;}
        public static <T extends Parcelling<?>> T put(T value){return value;}
    }
}
