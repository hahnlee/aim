// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

public abstract class SettingBase {
    SettingBase(int flags, int privateFlags) {}
    public SettingBase setFlags(int flags) { throw new RuntimeException("stub"); }
    public SettingBase setPrivateFlags(int flags) { throw new RuntimeException("stub"); }
    public com.android.server.pm.permission.LegacyPermissionState getLegacyPermissionState() { throw new RuntimeException("stub"); }
    public final void copySettingBase(SettingBase original) { throw new RuntimeException("stub"); }
    public int getFlags() { throw new RuntimeException("stub"); }
    public int getPrivateFlags() { throw new RuntimeException("stub"); }
}
