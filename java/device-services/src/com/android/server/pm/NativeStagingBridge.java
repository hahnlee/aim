package com.android.server.pm;

import android.apex.ApexSessionParams;
import android.content.pm.StagedApexInfo;
import android.os.Binder;
import android.os.Process;
import dev.aim.server.INativeStagingBridge;

/** Original apexd input owner; session readiness belongs to the native installer. */
public final class NativeStagingBridge extends INativeStagingBridge.Stub {
    @Override
    public StagedApexInfo[] getStagedApexInfos(int sessionId, int[] children) {
        if (Binder.getCallingUid() != Process.SYSTEM_UID)
            throw new SecurityException("native staging transport requires system UID");
        if (sessionId <= 0 || children == null)
            throw new IllegalArgumentException("invalid staged session");
        ApexSessionParams params = new ApexSessionParams();
        params.sessionId = sessionId;
        params.childSessionIds = children;
        var apexes = ApexManager.getInstance().getStagedApexInfos(params);
        var result = new StagedApexInfo[apexes.length];
        for (int i = 0; i < apexes.length; i++) {
            var apex = apexes[i];
            var info = new StagedApexInfo();
            info.moduleName = apex.moduleName;
            info.diskImagePath = apex.modulePath;
            info.versionCode = apex.versionCode;
            info.versionName = apex.versionName;
            info.hasClassPathJars = apex.hasClassPathJars;
            result[i] = info;
        }
        return result;
    }
}
