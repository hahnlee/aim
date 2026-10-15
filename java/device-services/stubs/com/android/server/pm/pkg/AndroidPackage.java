// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

public interface AndroidPackage {
    int getOverlayPriority();
    String getOverlayTarget();
    boolean isOverlayIsStatic();
    java.util.List<String> getLibraryNames();
    String getPackageName();
    String getPath();
    java.util.Set<String> getRequestedPermissions();
    long getLongVersionCode();
    int getTargetSdkVersion();
    boolean isPersistent();
    boolean isEnabled();
    boolean isApex();
    boolean isResourceOverlay();
    android.content.pm.SigningDetails getSigningDetails();
    String getBaseApkPath();
    java.util.List<com.android.internal.pm.pkg.component.ParsedProvider> getProviders();
    java.util.List<com.android.internal.pm.pkg.component.ParsedPermission> getPermissions();
    java.util.Map<String,android.content.pm.PackageManager.Property> getProperties();
    String getNativeLibraryDir();
    java.util.List<com.android.internal.pm.pkg.component.ParsedActivity> getReceivers();
    java.util.List<com.android.internal.pm.pkg.component.ParsedActivity> getActivities();
    java.util.List<com.android.internal.pm.pkg.component.ParsedService> getServices();
    boolean isSdkLibrary();
    boolean isStaticSharedLibrary();
    String getVolumeUuid();
    int getUid();
 int getPageSizeAppCompatFlags();
}
