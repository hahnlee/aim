// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

public interface ScanPackageUtils {
    public static java.util.List<String> applyAdjustedAbiToSharedUser(
            SharedUserSetting group, com.android.internal.pm.parsing.pkg.ParsedPackage scanned,
            String abi) { throw new RuntimeException("stub"); }
    public static void applyPolicy(
            com.android.internal.pm.parsing.pkg.ParsedPackage pkg, int scanFlags,
            com.android.server.pm.pkg.AndroidPackage platformPkg, boolean updatedSystemApp) {
        throw new RuntimeException("stub");
    }
}
