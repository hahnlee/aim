// Fixture-only static API; actual oracle references link against the image.
package com.android.server.pm;
public final class SELinuxMMAC {
    private SELinuxMMAC() {}
    public static boolean readInstallPolicy() { throw new RuntimeException("stub"); }
    public static String getSeInfo(com.android.server.pm.pkg.PackageState state,
            com.android.server.pm.pkg.AndroidPackage pkg, boolean privileged, int targetSdk) {
        throw new RuntimeException("stub");
    }
}
