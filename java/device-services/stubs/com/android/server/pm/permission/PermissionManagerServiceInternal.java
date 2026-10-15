// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.permission;
public interface PermissionManagerServiceInternal extends android.permission.PermissionManagerInternal, LegacyPermissionDataProvider {
    java.util.List<android.content.pm.PermissionInfo> getAllPermissionsWithProtection(int protection);
    boolean isPermissionsReviewRequired(String packageName, int userId);
    int[] getPermissionGids(String permissionName, int userId);
    void resetRuntimePermissions(com.android.server.pm.pkg.AndroidPackage pkg, int user);
    java.util.Set<String> getInstalledPermissions(String packageName);
    java.util.Set<String> getGrantedPermissions(String packageName, int userId);
    String getDefaultPermissionGrantFingerprint(int userId);
    void setDefaultPermissionGrantFingerprint(String fingerprint, int userId);
    void resetRuntimePermissionsForUser(int userId);
    void onSystemReady();
    void onUserCreated(int userId);
    void onUserRemoved(int userId);
    void readLegacyPermissionStateTEMP();
    void onPackageInstalled(com.android.server.pm.pkg.AndroidPackage pkg,int previousAppId,PackageInstalledParams params,int userId);
    final class PackageInstalledParams {
        private PackageInstalledParams(android.util.ArrayMap<String,Integer> states,java.util.List<String> permissions,int mode){throw new RuntimeException("stub");}
        public static final class Builder {
            public Builder(){throw new RuntimeException("stub");}
            public void setAllowlistedRestrictedPermissions(java.util.List<String> permissions){throw new RuntimeException("stub");}
            public Builder setPermissionStates(android.util.ArrayMap<String,Integer> states){throw new RuntimeException("stub");}
 public void setAutoRevokePermissionsMode(int mode){throw new RuntimeException("stub");}
 public PackageInstalledParams build(){throw new RuntimeException("stub");}
        }
    }
 HotwordDetectionServiceProvider getHotwordDetectionServiceProvider();
 interface HotwordDetectionServiceProvider {int getUid();}
 void onPackageAdded(com.android.server.pm.pkg.PackageState state,boolean instant,com.android.server.pm.pkg.AndroidPackage oldPkg);
 void readLegacyPermissionsTEMP(LegacyPermissionSettings settings);
 void writeLegacyPermissionsTEMP(LegacyPermissionSettings settings);
 void onPackageRemoved(com.android.server.pm.pkg.AndroidPackage pkg);
 void onPackageUninstalled(String name,int appId,com.android.server.pm.pkg.PackageState state,com.android.server.pm.pkg.AndroidPackage pkg,java.util.List<com.android.server.pm.pkg.AndroidPackage> shared,int user);
 void onStorageVolumeMounted(String volumeUuid,boolean fingerprintChanged);
}
