package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageItemInfo;

// Test-only seam for InstalledActivityInfoTest. The production projection pulls
// in Resources/Bundle metadata decoding that this record-mapping test does not
// exercise; InstalledApplicationInfoTest covers it separately.
final class InstalledApplicationInfo {
    private InstalledApplicationInfo() {}

    static void applyLabel(PackageItemInfo info, InstalledPackageRecord installed,
            String textKey, String resourceKey) {}

    static ApplicationInfo fromRecord(String packageName, String record) {
        ApplicationInfo info = new ApplicationInfo();
        info.packageName = packageName;
        return info;
    }
}
