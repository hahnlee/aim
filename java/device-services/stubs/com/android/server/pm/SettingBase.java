// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

public abstract class SettingBase {
    SettingBase(int flags, int privateFlags) {}
    public int getFlags() { throw new RuntimeException("stub"); }
    public int getPrivateFlags() { throw new RuntimeException("stub"); }
}
