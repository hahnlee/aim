// Compile-only pinned API; the original framework class runs in aim-system.
package android.app;
public abstract class ForegroundServiceTypePolicy {
    public static int FGS_TYPE_POLICY_CHECK_DISABLED, FGS_TYPE_POLICY_CHECK_PERMISSION_DENIED_ENFORCED,
            FGS_TYPE_POLICY_CHECK_PERMISSION_DENIED_PERMISSIVE, FGS_TYPE_POLICY_CHECK_DEPRECATED;
    public static ForegroundServiceTypePolicy getDefaultPolicy() { throw new RuntimeException("stub"); }
    public abstract ForegroundServiceTypePolicyInfo getForegroundServiceTypePolicyInfo(int type, int defaultToType);
    public abstract int checkForegroundServiceTypePolicy(android.content.Context context, String packageName,
            int uid, int pid, boolean allowWhileInUse, ForegroundServiceTypePolicyInfo policy);
    public static class ForegroundServiceTypePermissions {
        public ForegroundServiceTypePermissions(ForegroundServiceTypePermission[] permissions, boolean allOf) { throw new RuntimeException("stub"); }
    }
    public static final class ForegroundServiceTypePolicyInfo {
        public ForegroundServiceTypePolicyInfo(int type, long deprecation, long disabled,
                ForegroundServiceTypePermissions all, ForegroundServiceTypePermissions any,
                String flag, boolean enforced, boolean foregroundOnly) { throw new RuntimeException("stub"); }
        public int getForegroundServiceType() { throw new RuntimeException("stub"); }
        public void setCustomPermission(ForegroundServiceTypePermission permission) { throw new RuntimeException("stub"); }
        public String toPermissionString() { throw new RuntimeException("stub"); }
        public boolean hasForegroundOnlyPermission() { throw new RuntimeException("stub"); }
    }
    public abstract static class ForegroundServiceTypePermission {
        public ForegroundServiceTypePermission(String name) { throw new RuntimeException("stub"); }
        public abstract int checkPermission(android.content.Context context, int uid, int pid,
                String packageName, boolean allowWhileInUse);
    }
}
