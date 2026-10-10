// Compile-only pinned permission module API; never included in device-services.jar.
package com.android.permission.persistence;
public class RuntimePermissionsState {
    public RuntimePermissionsState(int version, String fingerprint, java.util.Map<String, java.util.List<PermissionState>> packages, java.util.Map<String, java.util.List<PermissionState>> shared) { throw new RuntimeException("stub"); }
    public int getVersion() { throw new RuntimeException("stub"); }
    public String getFingerprint() { throw new RuntimeException("stub"); }
    public java.util.Map<String, java.util.List<PermissionState>> getPackagePermissions() { throw new RuntimeException("stub"); }
    public java.util.Map<String, java.util.List<PermissionState>> getSharedUserPermissions() { throw new RuntimeException("stub"); }
    public static class PermissionState {
        public PermissionState(String name, boolean granted, int flags) { throw new RuntimeException("stub"); }
        public String getName() { throw new RuntimeException("stub"); }
        public boolean isGranted() { throw new RuntimeException("stub"); }
        public int getFlags() { throw new RuntimeException("stub"); }
    }
}
