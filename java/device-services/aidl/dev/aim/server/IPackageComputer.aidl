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
    boolean filterAppAccess(String packageName, int callingUid, int userId);
    void close();
}
