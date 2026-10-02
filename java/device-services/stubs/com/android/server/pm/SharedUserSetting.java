// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

public final class SharedUserSetting extends SettingBase {
    final PackageSignatures signatures = null;
    void addPackage(PackageSetting setting) { throw new RuntimeException("stub"); }
    SharedUserSetting(String name, int flags, int privateFlags) {
        super(flags, privateFlags);
    }
}
