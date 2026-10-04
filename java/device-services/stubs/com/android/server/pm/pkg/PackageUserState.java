// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

import android.content.pm.overlay.OverlayPaths;
import android.util.ArraySet;

public interface PackageUserState {
    PackageUserState DEFAULT = null;
    ArchiveState getArchiveState();
    long getCeDataInode();
    long getDeDataInode();
    ArraySet<String> getDisabledComponents();
    int getDistractionFlags();
    ArraySet<String> getEnabledComponents();
    int getEnabledState();
    long getFirstInstallTimeMillis();
    String getHarmfulAppWarning();
    int getInstallReason();
    String getLastDisableAppCaller();
    int getMinAspectRatio();
    OverlayPaths getAllOverlayPaths();
    OverlayPaths getOverlayPaths();
    java.util.Map<String, OverlayPaths> getSharedLibraryOverlayPaths();
    boolean isComponentEnabled(String componentName);
    boolean isComponentDisabled(String componentName);
    boolean dataExists();
    String getSplashScreenTheme();
    int getUninstallReason();
    boolean isHidden();
    boolean isInstalled();
    boolean isInstantApp();
    boolean isNotLaunched();
    boolean isQuarantined();
    boolean isStopped();
    boolean isSuspended();
    boolean isVirtualPreload();
}
