// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public class PackageSetting extends SettingBase {
    public PackageSetting snapshot() { throw new RuntimeException("stub"); }
    public PackageSetting setLeavingSharedUser(boolean value) { throw new RuntimeException("stub"); }
    public boolean isLeavingSharedUser() { throw new RuntimeException("stub"); }
    public com.android.server.pm.pkg.PackageUserStateImpl getOrCreateUserState(int id) { throw new RuntimeException("stub"); }
    public android.util.SparseArray<? extends com.android.server.pm.pkg.PackageUserStateInternal> getUserStates() { throw new RuntimeException("stub"); }
    public String[] getSplitNames() { throw new RuntimeException("stub"); }
    public int[] getSplitRevisionCodes() { throw new RuntimeException("stub"); }
    public String getRealName() { throw new RuntimeException("stub"); }
    public String getSecondaryCpuAbiLegacy() { throw new RuntimeException("stub"); }
    public String getAppMetadataFilePath() { throw new RuntimeException("stub"); }
    public int getAppMetadataSource() { throw new RuntimeException("stub"); }
    public int getBaseRevisionCode() { throw new RuntimeException("stub"); }

    public PackageSetting setLastModifiedTime(long value) { throw new RuntimeException("stub"); }
    public PackageSetting setLastUpdateTime(long value) { throw new RuntimeException("stub"); }
    public PackageSetting setLongVersionCode(long value) { throw new RuntimeException("stub"); }
    public PackageSetting setTargetSdkVersion(int value) { throw new RuntimeException("stub"); }
    public PackageSetting setRestrictUpdateHash(byte[] value) { throw new RuntimeException("stub"); }
    public PackageSetting setVolumeUuid(String value) { throw new RuntimeException("stub"); }
    public PackageSetting setCategoryOverride(int value) { throw new RuntimeException("stub"); }
    public PackageSetting setUpdateAvailable(boolean value) { throw new RuntimeException("stub"); }
    public PackageSetting setForceQueryableOverride(boolean value) { throw new RuntimeException("stub"); }
    public PackageSetting setPendingRestore(boolean value) { throw new RuntimeException("stub"); }
    public PackageSetting setDebuggable(boolean value) { throw new RuntimeException("stub"); }
    public PackageSetting setScannedAsStoppedSystemApp(boolean value) { throw new RuntimeException("stub"); }
    public PackageSetting setBaseRevisionCode(int value) { throw new RuntimeException("stub"); }
    public PackageSetting setAppMetadataFilePath(String value) { throw new RuntimeException("stub"); }
    public PackageSetting setAppMetadataSource(int value) { throw new RuntimeException("stub"); }

    void setEnabledComponentsCopy(com.android.server.utils.WatchedArraySet<String> components, int user) { throw new RuntimeException("stub"); }
    void setDisabledComponentsCopy(com.android.server.utils.WatchedArraySet<String> components, int user) { throw new RuntimeException("stub"); }
    public boolean isInstallPermissionsFixed() { throw new RuntimeException("stub"); }
    public PackageSetting setInstallPermissionsFixed(boolean fixed) { throw new RuntimeException("stub"); }
    public String getPackageName() { throw new RuntimeException("stub"); }
    public com.android.server.pm.pkg.PackageStateUnserialized getPkgState() { throw new RuntimeException("stub"); }
    public PackageSetting setPrimaryCpuAbi(String abi) { throw new RuntimeException("stub"); }
    public PackageSetting setSecondaryCpuAbi(String abi) { throw new RuntimeException("stub"); }
    public PackageSetting setPkg(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
    public PackageSetting setCpuAbiOverride(String abi) { throw new RuntimeException("stub"); }
    public String getCpuAbiOverride() { throw new RuntimeException("stub"); }
    public PackageSetting setLegacyNativeLibraryPath(String path) { throw new RuntimeException("stub"); }
    com.android.server.pm.pkg.PackageUserStateImpl modifyUserState(int user) { throw new RuntimeException("stub"); }
    void setInstalled(boolean installed, int user) { throw new RuntimeException("stub"); }
    void setUninstallReason(int reason, int user) { throw new RuntimeException("stub"); }
    int getUninstallReason(int user) { throw new RuntimeException("stub"); }
    public String getLegacyNativeLibraryPath() { throw new RuntimeException("stub"); }
    public String getPrimaryCpuAbiLegacy() { throw new RuntimeException("stub"); }
    public long getVersionCode() { throw new RuntimeException("stub"); }
    public PackageSetting addMimeTypes(String name, java.util.Set<String> values) { throw new RuntimeException("stub"); }
    public PackageSetting setUsesSdkLibraries(String[] values) { throw new RuntimeException("stub"); }
    public PackageSetting setUsesSdkLibrariesVersionsMajor(long[] values) { throw new RuntimeException("stub"); }
    public PackageSetting setUsesSdkLibrariesOptional(boolean[] values) { throw new RuntimeException("stub"); }
    public PackageSetting setUsesStaticLibraries(String[] values) { throw new RuntimeException("stub"); }
    public PackageSetting setUsesStaticLibrariesVersions(long[] values) { throw new RuntimeException("stub"); }
    public String[] getUsesSdkLibraries() { throw new RuntimeException("stub"); }
    public long[] getUsesSdkLibrariesVersionsMajor() { throw new RuntimeException("stub"); }
    public boolean[] getUsesSdkLibrariesOptional() { throw new RuntimeException("stub"); }
    public String[] getUsesStaticLibraries() { throw new RuntimeException("stub"); }
    public long[] getUsesStaticLibrariesVersions() { throw new RuntimeException("stub"); }
    public java.util.Map<String, java.util.Set<String>> getMimeGroups() { throw new RuntimeException("stub"); }
    public PackageSetting(String name, String realName, java.io.File path, int flags, int privateFlags, java.util.UUID domainSetId) {
        super(flags, privateFlags);
    }
    public android.content.pm.SigningDetails getSigningDetails() { throw new RuntimeException("stub"); }
    public PackageSetting setSigningDetails(android.content.pm.SigningDetails details) {
        throw new RuntimeException("stub");
    }
    public PackageSetting setAppId(int id) { throw new RuntimeException("stub"); }
    public PackageSetting setSharedUserAppId(int id) { throw new RuntimeException("stub"); }
    public PackageSetting setInstallSource(InstallSource source) { throw new RuntimeException("stub"); }
    public InstallSource getInstallSource() { throw new RuntimeException("stub"); }
    public PackageSetting setFirstInstallTime(long time, int user) { throw new RuntimeException("stub"); }
    public PackageSetting(PackageSetting original, boolean sealedSnapshot) { super(0, 0); }
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
    public PackageSetting setLoadingProgress(float value) { throw new RuntimeException("stub"); }
    public PackageSetting setLoadingCompletedTime(long value) { throw new RuntimeException("stub"); }
    public long getLoadingCompletedTime() { throw new RuntimeException("stub"); }
    public PackageSetting addOldPath(java.io.File value) { throw new RuntimeException("stub"); }
    public PackageSetting removeOldPath(java.io.File value) { throw new RuntimeException("stub"); }
    public java.util.LinkedHashSet<java.io.File> getOldPaths() { throw new RuntimeException("stub"); }
}
