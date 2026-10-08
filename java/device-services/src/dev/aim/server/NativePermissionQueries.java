package dev.aim.server;

import android.app.ActivityThread;
import android.app.AppOpsManager;
import android.content.Context;

/** Actual AppOps owner used by PMS.isAutoRevokeWhitelisted. */
public final class NativePermissionQueries {
    private NativePermissionQueries() {}
    public static boolean isAutoRevokeWhitelisted(int callingUid, String packageName) {
        Context context = ActivityThread.currentActivityThread().getSystemContext();
        AppOpsManager appOps = context.getSystemService(AppOpsManager.class);
        if (appOps == null) throw new IllegalStateException("app ops owner unavailable");
        return appOps.checkOpNoThrow(AppOpsManager.OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED,
                callingUid, packageName) == AppOpsManager.MODE_IGNORED;
    }
}
