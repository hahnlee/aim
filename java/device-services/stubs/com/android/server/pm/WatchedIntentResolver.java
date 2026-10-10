// Compile-only original image type; no runtime implementation.
package com.android.server.pm;
public abstract class WatchedIntentResolver<F extends WatchedIntentFilter, R extends WatchedIntentFilter> extends com.android.server.IntentResolver<F, R> {
    public void addFilter(com.android.server.pm.snapshot.PackageDataSnapshot snapshot,F filter) { throw new RuntimeException("stub"); }
}
