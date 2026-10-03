// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.pkg;
public class PackageStateUnserialized {
    public PackageStateUnserialized setUsesLibraryInfos(java.util.List<android.content.pm.SharedLibraryInfo> values) { throw new RuntimeException("stub"); }
    public PackageStateUnserialized setUsesLibraryFiles(java.util.List<String> values) { throw new RuntimeException("stub"); }
    public PackageStateUnserialized setSeInfo(String value) { throw new RuntimeException("stub"); }
    public PackageStateUnserialized setOverrideSeInfo(String value) { throw new RuntimeException("stub"); }
    public String getSeInfo() { throw new RuntimeException("stub"); }
    public String getOverrideSeInfo() { throw new RuntimeException("stub"); }
    public PackageStateUnserialized(com.android.server.pm.PackageSetting setting) { throw new RuntimeException("stub"); }
    public PackageStateUnserialized setUpdatedSystemApp(boolean value) { throw new RuntimeException("stub"); }
    public PackageStateUnserialized setLastPackageUsageTimeInMills(int reason, long time) { throw new RuntimeException("stub"); }
    public long[] getLastPackageUsageTimeInMills() { throw new RuntimeException("stub"); }
    public long getLatestPackageUseTimeInMills() { throw new RuntimeException("stub"); }
    public long getLatestForegroundPackageUseTimeInMills() { throw new RuntimeException("stub"); }
}
