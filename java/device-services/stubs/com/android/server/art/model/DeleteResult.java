// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.art.model;
public abstract class DeleteResult {
    protected DeleteResult() { throw new RuntimeException("stub"); }
    public abstract long getFreedBytes();
}
