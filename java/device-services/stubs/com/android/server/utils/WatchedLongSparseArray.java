// Compile-only original image type; no runtime implementation.
package com.android.server.utils;
public class WatchedLongSparseArray<T> extends WatchableImpl {
    public WatchedLongSparseArray() { throw new RuntimeException("stub"); }
    public void put(long key, T value) { throw new RuntimeException("stub"); }
    public T get(long key) { throw new RuntimeException("stub"); }
    public WatchedLongSparseArray<T> snapshot() { throw new RuntimeException("stub"); }
}
