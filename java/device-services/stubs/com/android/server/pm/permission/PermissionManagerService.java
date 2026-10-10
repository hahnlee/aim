// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.pm.permission;
public class PermissionManagerService extends android.permission.IPermissionManager.Stub {
    public android.content.pm.PermissionGroupInfo getPermissionGroupInfo(String name, int flags) { throw new RuntimeException("stub"); }
 public PermissionManagerService(android.content.Context context,android.util.ArrayMap<String,android.content.pm.FeatureInfo> features){throw new RuntimeException("stub");}
 public static PermissionManagerServiceInternal create(android.content.Context context,android.util.ArrayMap<String,android.content.pm.FeatureInfo> features){throw new RuntimeException("stub");}
}
