// A stub checked against the pinned image by the device-services node.
package com.android.server.compat;

public class PlatformCompat extends com.android.internal.compat.IPlatformCompat.Stub {
    public PlatformCompat(android.content.Context context) { throw new RuntimeException("stub"); }
    public boolean isChangeEnabledByUid(long changeId, int uid) { throw new RuntimeException("stub"); }
    public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) { throw new RuntimeException("stub"); }
    public com.android.internal.compat.CompatibilityChangeConfig getAppConfig(android.content.pm.ApplicationInfo info) { throw new RuntimeException("stub"); }
    public boolean isChangeEnabledInternalNoLogging(long changeId, android.content.pm.ApplicationInfo appInfo) { throw new RuntimeException("stub"); }
    public boolean isChangeEnabledInternal(long changeId, String packageName, int targetSdk) {
        throw new RuntimeException("stub");
    }
    public boolean isChangeEnabledInternal(long changeId, android.content.pm.ApplicationInfo appInfo) {
        throw new RuntimeException("stub");
    }
}
