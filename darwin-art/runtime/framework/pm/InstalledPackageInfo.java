package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;

/** Framework PackageInfo projection owned by the installed-package registry. */
public final class InstalledPackageInfo {
    private InstalledPackageInfo() {}

    public static PackageInfo fromRecord(String packageName, String record) {
        ApplicationInfo application = InstalledApplicationInfo.fromRecord(packageName, record);
        if (application == null) return null;

        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        PackageInfo info = new PackageInfo();
        info.packageName = packageName;
        info.applicationInfo = application;

        String versionCode = installed.legacyManifestHint("version_code");
        if (versionCode != null) {
            long parsed = Long.parseLong(versionCode);
            info.versionCode = (int) parsed;
            info.setLongVersionCode(parsed);
        }
        info.versionName = installed.legacyManifestHint("version_name");
        return info;
    }
}
