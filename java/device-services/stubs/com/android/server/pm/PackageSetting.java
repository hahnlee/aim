// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public class PackageSetting extends SettingBase {
    public PackageSetting(String name, String realName, java.io.File path, int flags, int privateFlags, java.util.UUID domainSetId) {
        super(flags, privateFlags);
    }
    public PackageSetting setSigningDetails(android.content.pm.SigningDetails details) {
        throw new RuntimeException("stub");
    }
}
