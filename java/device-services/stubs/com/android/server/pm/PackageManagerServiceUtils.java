// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public interface PackageManagerServiceUtils {
    public static boolean canJoinSharedUserId(String name, android.content.pm.SigningDetails details, SharedUserSetting group, int kind) {
        throw new RuntimeException("stub");
    }
}
