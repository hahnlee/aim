// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.RemoteException;

public interface IPackageManager extends android.os.IInterface {

    void setPackageStoppedState(String packageName, boolean stopped, int userId) throws RemoteException;
    void setComponentEnabledSetting(android.content.ComponentName component, int state, int flags, int userId, String callingPackage) throws RemoteException;
    int checkPermission(String permissionName,String packageName,int userId) throws RemoteException;
    PermissionGroupInfo getPermissionGroupInfo(String name, int flags) throws RemoteException;
    android.graphics.Bitmap getArchivedAppIcon(String name, android.os.UserHandle user, String callingPackage) throws RemoteException;
    boolean isAppArchivable(String name, android.os.UserHandle user) throws RemoteException;
    String getNameForUid(int uid) throws RemoteException;
    ApplicationInfo getApplicationInfo(String packageName, long flags, int userId) throws RemoteException;
    PackageInfo getPackageInfo(String packageName, long flags, int userId) throws RemoteException;
    String getSdkSandboxPackageName() throws RemoteException;
    ModuleInfo getModuleInfo(String packageName,int flags) throws RemoteException;
    java.util.List<ModuleInfo> getInstalledModules(int flags) throws RemoteException;
    boolean isUidPrivileged(int uid) throws RemoteException;
    ParceledListSlice<PackageInfo> getPackagesHoldingPermissions(String[] permissions, long flags, int userId) throws RemoteException;
    boolean isSafeMode() throws RemoteException;
    boolean isFirstBoot() throws RemoteException;
    boolean isDeviceUpgrading() throws RemoteException;
    void registerPackageMonitorCallback(android.os.IRemoteCallback callback, int userId) throws RemoteException;
    String getSuspendingPackage(String packageName, int userId) throws RemoteException;
    int[] getPackageGids(String packageName, long flags, int userId) throws RemoteException;
    InstallSourceInfo getInstallSourceInfo(String packageName, int userId) throws RemoteException;
    byte[] getPreferredActivityBackup(int userId) throws RemoteException;
    android.content.ComponentName getInstantAppResolverComponent() throws RemoteException;
    android.content.ComponentName getInstantAppInstallerComponent() throws RemoteException;
    boolean isPackageAvailable(String packageName, int userId) throws RemoteException;
    String[] currentToCanonicalPackageNames(String[] names) throws RemoteException;
    String[] canonicalToCurrentPackageNames(String[] names) throws RemoteException;
    ParceledListSlice<SharedLibraryInfo> getSharedLibraries(String packageName, long flags, int userId) throws RemoteException;
    ParceledListSlice<SharedLibraryInfo> getDeclaredSharedLibraries(String packageName, long flags, int userId) throws RemoteException;
    java.util.List<String> getAllPackages() throws RemoteException;
    String[] getNamesForUids(int[] uids) throws RemoteException;
    int getUidForSharedUser(String sharedUserName) throws RemoteException;
    int getFlagsForUid(int uid) throws RemoteException;
    int getPrivateFlagsForUid(int uid) throws RemoteException;
    int getInstallReason(String packageName, int userId) throws RemoteException;
    boolean getApplicationHiddenSettingAsUser(String packageName, int userId) throws RemoteException;
    boolean getBlockUninstallForUser(String packageName, int userId) throws RemoteException;
    int getApplicationEnabledSetting(String packageName, int userId) throws RemoteException;
    java.util.Map<String, String> getSystemSharedLibraryNamesAndPaths() throws RemoteException;
    int checkUidPermission(String permName, int uid) throws RemoteException;
    int checkSignatures(String pkg1, String pkg2, int userId) throws RemoteException;
    int checkUidSignatures(int uid1, int uid2) throws RemoteException;
    boolean hasSigningCertificate(String packageName, byte[] certificate, int type) throws RemoteException;
    boolean hasUidSigningCertificate(int uid, byte[] certificate, int type) throws RemoteException;
    KeySet getKeySetByAlias(String packageName, String alias) throws RemoteException;
    KeySet getSigningKeySet(String packageName) throws RemoteException;
    boolean isPackageSignedByKeySet(String packageName, KeySet ks) throws RemoteException;
    boolean isPackageSignedByKeySetExactly(String packageName, KeySet ks) throws RemoteException;
    ActivityInfo getActivityInfo(android.content.ComponentName component, long flags, int userId) throws RemoteException;
    ActivityInfo getReceiverInfo(android.content.ComponentName component, long flags, int userId) throws RemoteException;
    ProviderInfo resolveContentProviderForUid(String authority, long flags, int userId, int callingUid) throws RemoteException;
    ProviderInfo getProviderInfo(android.content.ComponentName component, long flags, int userId) throws RemoteException;
    ServiceInfo getServiceInfo(android.content.ComponentName component, long flags, int userId) throws RemoteException;
    InstrumentationInfo getInstrumentationInfoAsUser(android.content.ComponentName component, int flags, int userId) throws RemoteException;
    ParceledListSlice<InstrumentationInfo> queryInstrumentationAsUser(String targetPackage, int flags, int userId) throws RemoteException;
    ParceledListSlice<ProviderInfo> queryContentProviders(String processName, int uid, long flags, String metaDataKey) throws RemoteException;
    ParceledListSlice<PackageInfo> getInstalledPackages(long flags, int userId) throws RemoteException;
    String[] getAppOpPermissionPackages(String permissionName, int userId) throws RemoteException;
    CharSequence getHarmfulAppWarning(String packageName, int userId) throws RemoteException;
    boolean[] canPackageQuery(String sourcePackageName, String[] targetPackageNames, int userId) throws RemoteException;
    ParceledListSlice<android.content.IntentFilter> getAllIntentFilters(String packageName) throws RemoteException;
    abstract class Stub extends android.os.Binder implements IPackageManager {
        public static IPackageManager asInterface(android.os.IBinder binder) { throw new RuntimeException("stub"); }
    }
 boolean canForwardTo(android.content.Intent intent,String resolvedType,int sourceUser,int targetUser)throws android.os.RemoteException;
}
