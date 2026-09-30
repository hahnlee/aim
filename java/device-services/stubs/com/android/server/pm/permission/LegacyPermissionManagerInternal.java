// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.permission;

public interface LegacyPermissionManagerInternal {
    interface PackagesProvider {
        String[] getPackages(int userId);
    }

    void setLocationPackagesProvider(PackagesProvider provider);
    void setLocationExtraPackagesProvider(PackagesProvider provider);
    void grantDefaultPermissions(int userId);
}
