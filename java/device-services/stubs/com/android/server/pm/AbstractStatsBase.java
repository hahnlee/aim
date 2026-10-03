// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public abstract class AbstractStatsBase<T> {
    protected AbstractStatsBase(String file, String thread, boolean lock) { throw new RuntimeException("stub"); }
    protected android.util.AtomicFile getFile() { throw new RuntimeException("stub"); }
    protected abstract void readInternal(T data);
    protected abstract void writeInternal(T data);
}
