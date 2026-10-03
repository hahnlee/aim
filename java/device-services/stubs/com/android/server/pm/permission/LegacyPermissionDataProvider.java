// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.permission;
public interface LegacyPermissionDataProvider {
    int[] getGidsForUid(int uid);
}
