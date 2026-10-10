// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm;
abstract class AppsFilterLocked extends AppsFilterBase {
    protected boolean isImplicitlyQueryable(int recipient, int visible) { throw new RuntimeException("stub"); }
    protected boolean isRetainedImplicitlyQueryable(int recipient, int visible) { throw new RuntimeException("stub"); }
}
