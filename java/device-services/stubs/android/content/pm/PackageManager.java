// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.UserHandle;

public abstract class PackageManager {
    public static final int MATCH_KNOWN_PACKAGES = 0x402000;
    public static final int MATCH_HIDDEN_UNTIL_INSTALLED_COMPONENTS=0x20000000, MATCH_DISABLED_UNTIL_USED_COMPONENTS=0x8000;

    public static final boolean APP_ENUMERATION_ENABLED_BY_DEFAULT = true;
    public static String EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT;
    public static int installStatusToPublicStatus(int status) { throw new RuntimeException("stub"); }
    public static String installStatusToString(int status, String message) { throw new RuntimeException("stub"); }
    public static final String ACTION_REQUEST_PERMISSIONS_FOR_OTHER = "android.content.pm.action.REQUEST_PERMISSIONS_FOR_OTHER";
    public static final String EXTRA_REQUEST_PERMISSIONS_NAMES = "android.content.pm.extra.REQUEST_PERMISSIONS_NAMES";
    public static final String EXTRA_REQUEST_PERMISSIONS_RESULTS = "android.content.pm.extra.REQUEST_PERMISSIONS_RESULTS";
    public static final int VERSION_CODE_HIGHEST = -1;
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

    public static void invalidatePackageInfoCache() { throw new RuntimeException("stub"); }

    public static class NameNotFoundException extends android.util.AndroidException {
        public NameNotFoundException() {}
        public NameNotFoundException(String name) {}
    }

    public abstract ApplicationInfo getApplicationInfo(String packageName, int flags) throws NameNotFoundException;

    public abstract PackageInfo getPackageInfoAsUser(String packageName, int flags, int userId) throws NameNotFoundException;
    public abstract int getPermissionFlags(String permName, String packageName, UserHandle user);
    public abstract boolean hasSystemFeature(String featureName);
    public abstract java.util.List<PermissionGroupInfo> getAllPermissionGroups(int flags);
    public abstract java.util.List<PermissionInfo> queryPermissionsByGroup(String permissionGroup, int flags) throws NameNotFoundException;
    public abstract PermissionInfo getPermissionInfo(String permissionName, int flags) throws NameNotFoundException;
    public abstract PackageInstaller getPackageInstaller();
    public PackageInfo getPackageArchiveInfo(String archiveFilePath, int flags) { throw new RuntimeException("stub"); }
    public static final class Property {
        public Property(String name,boolean value,String packageName,String className){throw new RuntimeException("stub");}
        public boolean getBoolean(){throw new RuntimeException("stub");}
    }
    public abstract String getPermissionControllerPackageName();
    public abstract int checkPermission(String permission, String packageName);
 public static final int INSTALL_SCENARIO_DEFAULT=0,INSTALL_SCENARIO_FAST=1,INSTALL_SCENARIO_BULK=2,INSTALL_SCENARIO_BULK_SECONDARY=3;
 public static final int INSTALL_REASON_DEVICE_RESTORE=2,INSTALL_REASON_DEVICE_SETUP=3;
 public static int INSTALL_IGNORE_DEXOPT_PROFILE=268435456,INSTALL_GRANT_ALL_REQUESTED_PERMISSIONS=256,INSTALL_ALL_WHITELIST_RESTRICTED_PERMISSIONS=4194304,INSTALL_INSTANT_APP=2048,DELETE_ARCHIVE=16;
 public static boolean APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE=true;
 public static int deleteStatusToPublicStatus(int status){throw new RuntimeException("stub");}
 public static String deleteStatusToString(int status,String message){throw new RuntimeException("stub");}
 public static class UninstallCompleteCallback implements android.os.Parcelable {
 public UninstallCompleteCallback(android.os.IBinder binder){throw new RuntimeException("stub");}
 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel out,int flags){throw new RuntimeException("stub");}}

 public ApplicationInfo getApplicationInfoAsUser(String name,int flags,int user)throws NameNotFoundException{throw new RuntimeException("stub");}
 public CharSequence getApplicationLabel(ApplicationInfo info){throw new RuntimeException("stub");}
}
