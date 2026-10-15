package com.android.server.pm;

/** The original APEX owner alone controls unstagePackages; selection is native. */
public final class NativePackageApexUninstallBridge extends dev.aim.server.IPackageApexUninstallBridge.Stub {
    @Override public boolean unstagePackage(String path) {
        if (android.os.Binder.getCallingUid()!=android.os.Process.SYSTEM_UID) throw new SecurityException("native APEX uninstall requires system UID");
        return ApexManager.getInstance().uninstallApex(java.util.Objects.requireNonNull(path));
    }
}
