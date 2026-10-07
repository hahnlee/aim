package dev.aim.server;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;

/** Immutable native query capture. System UID transport; original caller is explicit. */
interface IPackageComputer {
    long getVersion();
    ApplicationInfo getApplicationInfo(String packageName, long flags, int userId,
            int filterCallingUid, int callingUid, int callingPid);
    PackageInfo getPackageInfo(String packageName, long flags, int userId,
            int filterCallingUid, int callingUid, int callingPid);
    boolean filterAppAccess(String packageName, int callingUid, int userId, boolean filterUninstalled);
    void close();
    int getPackageUid(String packageName, long flags, int userId, int callingUid, int callingPid);
    String[] getPackagesForUid(int uid, int callingUid, int callingPid);
    String getNameForUid(int uid, int callingUid, int callingPid);
    boolean isInstantApp(String packageName, int userId, int callingUid, int callingPid);
    int getTargetSdkVersion(String packageName, int callingUid, int callingPid);
    String getInstallerPackageName(String packageName, int userId, int callingUid, int callingPid);
    /** Original PackageManagerInternal UID lookup: SYSTEM_UID filter, without public enforcement. */
    int getPackageUidInternal(String packageName, long flags, int userId);
    /** Original renamed/static-library normalization, before a captured parsed-package lookup. */
    String resolveInternalPackageName(String packageName, long versionCode, int callingUid);
}
