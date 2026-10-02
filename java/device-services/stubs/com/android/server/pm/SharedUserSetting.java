// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

public final class SharedUserSetting extends SettingBase {
    SharedUserSetting(String name, int flags, int privateFlags) {
        super(flags, privateFlags);
    }
}
