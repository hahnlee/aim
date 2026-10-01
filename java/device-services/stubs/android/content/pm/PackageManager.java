// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.UserHandle;

public abstract class PackageManager {
    public static final String ACTION_REQUEST_PERMISSIONS_FOR_OTHER = "android.content.pm.action.REQUEST_PERMISSIONS_FOR_OTHER";
    public static final String EXTRA_REQUEST_PERMISSIONS_NAMES = "android.content.pm.extra.REQUEST_PERMISSIONS_NAMES";
    public static final String EXTRA_REQUEST_PERMISSIONS_RESULTS = "android.content.pm.extra.REQUEST_PERMISSIONS_RESULTS";
    public static final int PERMISSION_GRANTED = 0;
    public static final int PERMISSION_DENIED = -1;
    public static final int GET_PERMISSIONS = 4096;
    public static final int GET_PROVIDERS = 8;
    public static final int MATCH_DISABLED_COMPONENTS = 512;
    public static final int MATCH_DIRECT_BOOT_UNAWARE = 262144;
    public static final int MATCH_DIRECT_BOOT_AWARE = 524288;
    public static final int FLAG_PERMISSION_POLICY_FIXED = 4;
    public static final int FLAG_PERMISSION_SYSTEM_FIXED = 16;
    public static final String FEATURE_TELEVISION = "android.hardware.type.television";
    public static final String FEATURE_LEANBACK = "android.software.leanback";
    public static final String FEATURE_AUTOMOTIVE = "android.hardware.type.automotive";
    public static final String FEATURE_WATCH = "android.hardware.type.watch";

    public static class NameNotFoundException extends android.util.AndroidException {}

    public abstract ApplicationInfo getApplicationInfo(String packageName, int flags) throws NameNotFoundException;

    public abstract PackageInfo getPackageInfoAsUser(String packageName, int flags, int userId) throws NameNotFoundException;
    public abstract int getPermissionFlags(String permName, String packageName, UserHandle user);
    public abstract boolean hasSystemFeature(String featureName);
    public abstract java.util.List<PermissionGroupInfo> getAllPermissionGroups(int flags);
    public abstract java.util.List<PermissionInfo> queryPermissionsByGroup(String permissionGroup, int flags) throws NameNotFoundException;
    public abstract PackageInstaller getPackageInstaller();
    public PackageInfo getPackageArchiveInfo(String archiveFilePath, int flags) { throw new RuntimeException("stub"); }
}
