package com.android.server.pm;

import android.app.ActivityThread;
import android.app.admin.SecurityLog;
import android.content.pm.PackageManagerInternal;
import com.android.server.LocalServices;
import dev.aim.server.NativePackageManagerInternal;

/** Retains the original security logging/checksum helper below native PMS. */
public final class NativeProcessLoggingBridge {
    private static ProcessLoggingHandler handler;
    private NativeProcessLoggingBridge() {}
    public static void log(String packageName, String processName, int uid,
            String seinfo, String apkFile, int pid) {
        if (!SecurityLog.isLoggingEnabled()) return;
        PackageManagerInternal packages = LocalServices.getService(PackageManagerInternal.class);
        if (!(packages instanceof NativePackageManagerInternal))
            throw new IllegalStateException("native package internal owner unavailable");
        ProcessLoggingHandler current;
        synchronized (NativeProcessLoggingBridge.class) {
            if (handler == null) handler = new ProcessLoggingHandler();
            current = handler;
        }
        current.logAppProcessStart(ActivityThread.currentActivityThread().getSystemContext(),
                packages, apkFile, packageName, processName, uid, seinfo, pid);
    }
    public static synchronized void invalidate(String apkFile) {
        if (handler != null) handler.invalidateBaseApkHash(apkFile);
    }
}
