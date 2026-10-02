// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public class PackageSetting extends SettingBase {
    public String getPackageName() { throw new RuntimeException("stub"); }
    public com.android.server.pm.pkg.PackageStateUnserialized getPkgState() { throw new RuntimeException("stub"); }
    public PackageSetting setPrimaryCpuAbi(String abi) { throw new RuntimeException("stub"); }
    public PackageSetting setSecondaryCpuAbi(String abi) { throw new RuntimeException("stub"); }
    public PackageSetting setPkg(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
    public PackageSetting setCpuAbiOverride(String abi) { throw new RuntimeException("stub"); }
    public String getCpuAbiOverride() { throw new RuntimeException("stub"); }
    public PackageSetting setLegacyNativeLibraryPath(String path) { throw new RuntimeException("stub"); }
    void setInstalled(boolean installed, int user) { throw new RuntimeException("stub"); }
    void setUninstallReason(int reason, int user) { throw new RuntimeException("stub"); }
    int getUninstallReason(int user) { throw new RuntimeException("stub"); }
    public String getLegacyNativeLibraryPath() { throw new RuntimeException("stub"); }
    public String getPrimaryCpuAbiLegacy() { throw new RuntimeException("stub"); }
    public long getVersionCode() { throw new RuntimeException("stub"); }
    public java.util.Map<String, java.util.Set<String>> getMimeGroups() { throw new RuntimeException("stub"); }
    public PackageSetting(String name, String realName, java.io.File path, int flags, int privateFlags, java.util.UUID domainSetId) {
        super(flags, privateFlags);
    }
    public PackageSetting setSigningDetails(android.content.pm.SigningDetails details) {
        throw new RuntimeException("stub");
    }
    public PackageSetting setAppId(int id) { throw new RuntimeException("stub"); }
    public int getAppId() { throw new RuntimeException("stub"); }
    public int getCategoryOverride() { throw new RuntimeException("stub"); }
    public int getPageSizeAppCompatFlags() { throw new RuntimeException("stub"); }
    public PackageSetting setPageSizeAppCompatFlags(int flags) { throw new RuntimeException("stub"); }
    public float getLoadingProgress() { throw new RuntimeException("stub"); }
    public boolean isLoading() { throw new RuntimeException("stub"); }
    public PackageKeySetData getKeySetData() { throw new RuntimeException("stub"); }
    public java.util.UUID getDomainSetId() { throw new RuntimeException("stub"); }
    public boolean hasSharedUser() { throw new RuntimeException("stub"); }
    public boolean isScannedAsStoppedSystemApp() { throw new RuntimeException("stub"); }
    public com.android.server.pm.pkg.PackageUserStateInternal readUserState(int user) { throw new RuntimeException("stub"); }
    boolean getInstalled(int user) { throw new RuntimeException("stub"); }
    public boolean getInstantApp(int user) { throw new RuntimeException("stub"); }
    boolean getVirtualPreload(int user) { throw new RuntimeException("stub"); }
}
