// Compile-only image API; checked by the device-services build node.
package android.content.pm.pkg;
public interface FrameworkPackageUserState {
    android.content.pm.overlay.OverlayPaths getAllOverlayPaths();
    long getCeDataInode();
    java.util.Set<String> getDisabledComponents();
    int getDistractionFlags();
    java.util.Set<String> getEnabledComponents();
    int getEnabledState();
    String getHarmfulAppWarning();
    int getInstallReason();
    String getLastDisableAppCaller();
    android.content.pm.overlay.OverlayPaths getOverlayPaths();
    java.util.Map<String, android.content.pm.overlay.OverlayPaths> getSharedLibraryOverlayPaths();
    int getUninstallReason();
    boolean isComponentEnabled(String componentName);
    boolean isComponentDisabled(String componentName);
    boolean isHidden(); boolean isInstalled(); boolean isInstantApp();
    boolean isNotLaunched(); boolean isStopped(); boolean isSuspended(); boolean isVirtualPreload();
    String getSplashScreenTheme();
}
