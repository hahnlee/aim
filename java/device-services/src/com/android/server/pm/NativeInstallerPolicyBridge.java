package com.android.server.pm;

import com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags;
import android.os.Binder;
import android.os.Build;
import dev.aim.server.IInstallerPolicyBridge;

/** Original non-PMS owners for PackageInstallerService.createSessionInternal. */
public final class NativeInstallerPolicyBridge extends IInstallerPolicyBridge.Stub {
    @Override public int getPolicyFlags() {
        int uid = Binder.getCallingUid();
        if (uid != 0 && uid != 1000) throw new SecurityException("untrusted installer policy caller");
        return (Build.IS_DEBUGGABLE ? 1 : 0)
                | (ApexManager.getInstance().isApexSupported() ? 2 : 0)
                | (Flags.rollbackLifetime() ? 4 : 0);
    }
}
