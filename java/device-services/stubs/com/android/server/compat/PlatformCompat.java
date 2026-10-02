// A stub checked against the pinned image by the device-services node.
package com.android.server.compat;

public abstract class PlatformCompat extends com.android.internal.compat.IPlatformCompat.Stub {
    public PlatformCompat(android.content.Context context) { throw new RuntimeException("stub"); }
    public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) {
        throw new RuntimeException("stub");
    }
    public boolean isChangeEnabledInternal(long changeId, String packageName, int targetSdk) {
        throw new RuntimeException("stub");
    }
}
