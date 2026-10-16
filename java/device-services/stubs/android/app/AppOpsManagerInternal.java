// Compile-only pinned image API; runtime uses the original class.
package android.app;
public abstract class AppOpsManagerInternal {
    public abstract void onPackageAdded(String pkgName, int uid);
}
