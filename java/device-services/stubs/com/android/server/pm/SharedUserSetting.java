// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

public final class SharedUserSetting extends SettingBase {
    int mAppId;
    void addProcesses(java.util.Map<String, com.android.internal.pm.pkg.component.ParsedProcess> processes) { throw new RuntimeException("stub"); }
    public void updateProcesses() { throw new RuntimeException("stub"); }
    final android.util.ArrayMap<String, com.android.internal.pm.pkg.component.ParsedProcess> processes = null;
    final PackageSignatures signatures = null;
    boolean removePackage(PackageSetting setting) { throw new RuntimeException("stub"); }
    public int getSeInfoTargetSdkVersion() { throw new RuntimeException("stub"); }
    public void fixSeInfoLocked() { throw new RuntimeException("stub"); }
    void addPackage(PackageSetting setting) { throw new RuntimeException("stub"); }
    public android.util.ArraySet<? extends com.android.server.pm.pkg.PackageStateInternal> getPackageStates() {
        throw new RuntimeException("stub");
    }
    SharedUserSetting(String name, int flags, int privateFlags) {
        super(flags, privateFlags);
    }
}
