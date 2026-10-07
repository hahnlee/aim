// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

public interface AndroidPackage {
    String getPackageName();
    String getPath();
    long getLongVersionCode();
    int getTargetSdkVersion();
    boolean isPersistent();
    android.content.pm.SigningDetails getSigningDetails();
    String getBaseApkPath();
    java.util.List<com.android.internal.pm.pkg.component.ParsedProvider> getProviders();
    java.util.List<com.android.internal.pm.pkg.component.ParsedPermission> getPermissions();
}
