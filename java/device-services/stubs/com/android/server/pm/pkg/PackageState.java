// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

import android.content.pm.SigningInfo;
import android.util.SparseArray;

import java.io.File;
import java.util.List;
import java.util.Map;
import java.util.Set;

public interface PackageState {
    AndroidPackage getAndroidPackage();
    PackageUserState getStateForUser(android.os.UserHandle user);
    long[] getLastPackageUsageTime();
    String getPageSizeCompatWarningMessage(android.content.Context context);
    default PackageUserState getUserStateOrDefault(int user) {
        PackageUserState state = getUserStates().get(user);
        return state == null ? PackageUserState.DEFAULT : state;
    }
    String getApexModuleName();
    int getAppId();
    int getCategoryOverride();
    String getCpuAbiOverride();
    int getHiddenApiEnforcementPolicy();
    long getLastModifiedTime();
    long getLastUpdateTime();
    Map<String, Set<String>> getMimeGroups();
    String getPackageName();
    File getPath();
    String getPrimaryCpuAbi();
    byte[] getRestrictUpdateHash();
    String getSeInfo();
    String getSecondaryCpuAbi();
    List<SharedLibrary> getSharedLibraryDependencies();
    int getSharedUserAppId();
    SigningInfo getSigningInfo();
    int getTargetSdkVersion();
    SparseArray<? extends PackageUserState> getUserStates();
    List<String> getUsesLibraryFiles();
    String[] getUsesSdkLibraries();
    boolean[] getUsesSdkLibrariesOptional();
    long[] getUsesSdkLibrariesVersionsMajor();
    String[] getUsesStaticLibraries();
    long[] getUsesStaticLibrariesVersions();
    long getVersionCode();
    String getVolumeUuid();
    boolean hasSharedUser();
    boolean isApex();
    boolean isApkInUpdatedApex();
    boolean isDebuggable();
    boolean isDefaultToDeviceProtectedStorage();
    boolean isExternalStorage();
    boolean isForceQueryableOverride();
    boolean isHiddenUntilInstalled();
    boolean isInstallPermissionsFixed();
    boolean isLeavingSharedUser();
    boolean isOdm();
    boolean isOem();
    boolean isPageSizeAppCompatEnabled();
    boolean isPendingRestore();
    boolean isPersistent();
    boolean isPrivileged();
    boolean isProduct();
    boolean isRequiredForSystemUser();
    boolean isScannedAsStoppedSystemApp();
    boolean isSystem();
    boolean isSystemExt();
    boolean isUpdateAvailable();
    boolean isUpdatedSystemApp();
    boolean isVendor();
}
