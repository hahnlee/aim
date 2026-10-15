// Compile-only pinned image API; checked against the original image.
package com.android.server.pm.pkg;
public final class SuspendParams {
    public SuspendParams(android.content.pm.SuspendDialogInfo dialog, android.os.PersistableBundle appExtras, android.os.PersistableBundle launcherExtras, boolean quarantined) {
        throw new RuntimeException("stub");
    }
    public static SuspendParams restoreFromXml(com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException {
        throw new RuntimeException("stub");
    }
    public android.content.pm.SuspendDialogInfo getDialogInfo() { throw new RuntimeException("stub"); }
    public android.os.PersistableBundle getAppExtras() { throw new RuntimeException("stub"); }
    public android.os.PersistableBundle getLauncherExtras() { throw new RuntimeException("stub"); }
    public boolean isQuarantined() { throw new RuntimeException("stub"); }
}
