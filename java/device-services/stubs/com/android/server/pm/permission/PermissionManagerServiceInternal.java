// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.permission;
public interface PermissionManagerServiceInternal extends LegacyPermissionDataProvider {
    java.util.Set<String> getInstalledPermissions(String packageName);
    java.util.Set<String> getGrantedPermissions(String packageName, int userId);
}
