// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public interface PackageManagerServiceUtils {
    public static String deriveAbiOverride(String abi) { throw new RuntimeException("stub"); }
    public static long getLastModifiedTime(com.android.server.pm.pkg.AndroidPackage pkg) {
        throw new RuntimeException("stub");
    }
    public static boolean canJoinSharedUserId(String name, android.content.pm.SigningDetails details, SharedUserSetting group, int kind) {
        throw new RuntimeException("stub");
    }
}
