// Host-only Android boundary double; production policy is compiled unchanged.
package android.content.pm;
public abstract class PackageManager {
    public static final int GET_SIGNING_CERTIFICATES = 0x08000000;
    public static final int MATCH_UNINSTALLED_PACKAGES = 0x00002000;
    public static final int MATCH_ANY_USER = 0x00400000;
    public static final int MATCH_STATIC_SHARED_AND_SDK_LIBRARIES = 0x04000000;
    public static final int MATCH_APEX = 0x40000000;
    public static class NameNotFoundException extends Exception {}
    public abstract java.util.List<PackageInfo> getInstalledPackagesAsUser(int flags,int user);
    public abstract java.util.List<PackageInfo> getInstalledPackages(int flags);
    public abstract PackageInfo getPackageInfo(String name,int flags) throws NameNotFoundException;
    public abstract InstallSourceInfo getInstallSourceInfo(String name) throws NameNotFoundException;
}
