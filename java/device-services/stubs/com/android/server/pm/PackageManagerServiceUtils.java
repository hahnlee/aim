// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public abstract class PackageManagerServiceUtils {
    private PackageManagerServiceUtils() { throw new RuntimeException("stub"); }
    public static android.content.Intent updateIntentForResolve(android.content.Intent intent) { throw new RuntimeException("stub"); }
    public static java.io.File preparePackageParserCache(boolean eng, boolean userdebug, String incremental) { throw new RuntimeException("stub"); }
    public static String deriveAbiOverride(String abi) { throw new RuntimeException("stub"); }
    public static long getLastModifiedTime(com.android.server.pm.pkg.AndroidPackage pkg) {
        throw new RuntimeException("stub");
    }
    public static boolean canJoinSharedUserId(String name, android.content.pm.SigningDetails details, SharedUserSetting group, int kind) {
        throw new RuntimeException("stub");
    }
 public static void enforceShellRestriction(UserManagerInternal users,String restriction,int uid,int user){throw new RuntimeException("stub");}
}
