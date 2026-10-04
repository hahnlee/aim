package dev.aim.server;

import android.content.pm.SigningDetails;
import android.content.pm.SigningInfo;
import android.util.SparseArray;
import com.android.internal.pm.parsing.pkg.AndroidPackageInternal;
import com.android.server.pm.InstallSource;
import com.android.server.pm.PackageKeySetData;
import com.android.server.pm.PackageSetting;
import com.android.server.pm.permission.LegacyPermissionState;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.PackageStateUnserialized;
import com.android.server.pm.pkg.PackageUserState;
import com.android.server.pm.pkg.PackageUserStateInternal;
import com.android.server.pm.pkg.SharedLibrary;
import java.io.File;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.function.Supplier;

/** One native capture exposed through the pinned original read-only interface. */
public final class PackageStateReplica implements PackageStateInternal {
    private final Supplier<PackageSetting> factory;
    private final PackageStateInternal state;
    private final Map<Integer, PackageUserStateInternal> users;
    private final int hiddenApiPolicy;

    PackageStateReplica(Supplier<PackageSetting> factory, Map<Integer, PackageUserStateReplica> users,
            int hiddenApiPolicy) {
        this.factory = factory;
        this.state = fresh();
        this.users = java.util.Collections.unmodifiableMap(new java.util.LinkedHashMap<>(users));
        this.hiddenApiPolicy = hiddenApiPolicy;
    }

    private PackageStateInternal fresh() { return (PackageStateInternal) factory.get(); }

    @Override public AndroidPackage getAndroidPackage() { return state.getAndroidPackage(); }

    @Override public PackageUserState getStateForUser(android.os.UserHandle user) { return getUserStateOrDefault(user.getIdentifier()); }

    @Override public long[] getLastPackageUsageTime() {
        var value = state.getLastPackageUsageTime();
        return value == null ? null : value.clone();
    }

    @Override public String getPageSizeCompatWarningMessage(android.content.Context context) { return state.getPageSizeCompatWarningMessage(context); }

    @Override public String getApexModuleName() { return state.getApexModuleName(); }

    @Override public int getAppId() { return state.getAppId(); }

    @Override public int getCategoryOverride() { return state.getCategoryOverride(); }

    @Override public String getCpuAbiOverride() { return state.getCpuAbiOverride(); }

    @Override public int getHiddenApiEnforcementPolicy() { return hiddenApiPolicy; }

    @Override public long getLastModifiedTime() { return state.getLastModifiedTime(); }

    @Override public long getLastUpdateTime() { return state.getLastUpdateTime(); }

    @Override public Map<String, Set<String>> getMimeGroups() {
        var result = new java.util.LinkedHashMap<String, Set<String>>();
        for (var entry : state.getMimeGroups().entrySet()) {
            result.put(entry.getKey(), java.util.Collections.unmodifiableSet(new java.util.LinkedHashSet<>(entry.getValue())));
        }
        return java.util.Collections.unmodifiableMap(result);
    }

    @Override public String getPackageName() { return state.getPackageName(); }

    @Override public File getPath() { return state.getPath(); }

    @Override public String getPrimaryCpuAbi() { return state.getPrimaryCpuAbi(); }

    @Override public byte[] getRestrictUpdateHash() {
        var value = state.getRestrictUpdateHash();
        return value == null ? null : value.clone();
    }

    @Override public String getSeInfo() { return state.getSeInfo(); }

    @Override public String getSecondaryCpuAbi() { return state.getSecondaryCpuAbi(); }

    @Override public List<SharedLibrary> getSharedLibraryDependencies() { return java.util.Collections.unmodifiableList(new java.util.ArrayList<>(fresh().getSharedLibraryDependencies())); }

    @Override public int getSharedUserAppId() { return state.getSharedUserAppId(); }

    @Override public SigningInfo getSigningInfo() { return fresh().getSigningInfo(); }

    @Override public int getTargetSdkVersion() { return state.getTargetSdkVersion(); }

    @Override public SparseArray<? extends PackageUserStateInternal> getUserStates() {
        var result = new SparseArray<PackageUserStateInternal>();
        for (var entry : users.entrySet()) result.put(entry.getKey(), entry.getValue());
        return result;
    }

    @Override public List<String> getUsesLibraryFiles() {
        var value = state.getUsesLibraryFiles();
        return value == null ? null : java.util.Collections.unmodifiableList(new java.util.ArrayList<>(value));
    }

    @Override public String[] getUsesSdkLibraries() {
        var value = state.getUsesSdkLibraries();
        return value == null ? null : value.clone();
    }

    @Override public boolean[] getUsesSdkLibrariesOptional() {
        var value = state.getUsesSdkLibrariesOptional();
        return value == null ? null : value.clone();
    }

    @Override public long[] getUsesSdkLibrariesVersionsMajor() {
        var value = state.getUsesSdkLibrariesVersionsMajor();
        return value == null ? null : value.clone();
    }

    @Override public String[] getUsesStaticLibraries() {
        var value = state.getUsesStaticLibraries();
        return value == null ? null : value.clone();
    }

    @Override public long[] getUsesStaticLibrariesVersions() {
        var value = state.getUsesStaticLibrariesVersions();
        return value == null ? null : value.clone();
    }

    @Override public long getVersionCode() { return state.getVersionCode(); }

    @Override public String getVolumeUuid() { return state.getVolumeUuid(); }

    @Override public boolean hasSharedUser() { return state.hasSharedUser(); }

    @Override public boolean isApex() { return state.isApex(); }

    @Override public boolean isApkInUpdatedApex() { return state.isApkInUpdatedApex(); }

    @Override public boolean isDebuggable() { return state.isDebuggable(); }

    @Override public boolean isDefaultToDeviceProtectedStorage() { return state.isDefaultToDeviceProtectedStorage(); }

    @Override public boolean isExternalStorage() { return state.isExternalStorage(); }

    @Override public boolean isForceQueryableOverride() { return state.isForceQueryableOverride(); }

    @Override public boolean isHiddenUntilInstalled() { return state.isHiddenUntilInstalled(); }

    @Override public boolean isInstallPermissionsFixed() { return state.isInstallPermissionsFixed(); }

    @Override public boolean isLeavingSharedUser() { return state.isLeavingSharedUser(); }

    @Override public boolean isOdm() { return state.isOdm(); }

    @Override public boolean isOem() { return state.isOem(); }

    @Override public boolean isPageSizeAppCompatEnabled() { return state.isPageSizeAppCompatEnabled(); }

    @Override public boolean isPendingRestore() { return state.isPendingRestore(); }

    @Override public boolean isPersistent() { return state.isPersistent(); }

    @Override public boolean isPrivileged() { return state.isPrivileged(); }

    @Override public boolean isProduct() { return state.isProduct(); }

    @Override public boolean isRequiredForSystemUser() { return state.isRequiredForSystemUser(); }

    @Override public boolean isScannedAsStoppedSystemApp() { return state.isScannedAsStoppedSystemApp(); }

    @Override public boolean isSystem() { return state.isSystem(); }

    @Override public boolean isSystemExt() { return state.isSystemExt(); }

    @Override public boolean isUpdateAvailable() { return state.isUpdateAvailable(); }

    @Override public boolean isUpdatedSystemApp() { return state.isUpdatedSystemApp(); }

    @Override public boolean isVendor() { return state.isVendor(); }

    @Override public AndroidPackageInternal getPkg() { return state.getPkg(); }

    @Override public PackageStateUnserialized getTransientState() { return fresh().getTransientState(); }

    @Override public UUID getDomainSetId() { return state.getDomainSetId(); }

    @Override public SigningDetails getSigningDetails() { return fresh().getSigningDetails(); }

    @Override public InstallSource getInstallSource() { return fresh().getInstallSource(); }

    @Override public int getFlags() { return state.getFlags(); }

    @Override public int getPrivateFlags() { return state.getPrivateFlags(); }

    @Override public LegacyPermissionState getLegacyPermissionState() { return fresh().getLegacyPermissionState(); }

    @Override public String getRealName() { return state.getRealName(); }

    @Override public boolean isLoading() { return state.isLoading(); }

    @Override public String getPathString() { return state.getPathString(); }

    @Override public float getLoadingProgress() { return state.getLoadingProgress(); }

    @Override public long getLoadingCompletedTime() { return state.getLoadingCompletedTime(); }

    @Override public PackageKeySetData getKeySetData() { return fresh().getKeySetData(); }

    @Override public String getPrimaryCpuAbiLegacy() { return state.getPrimaryCpuAbiLegacy(); }

    @Override public String getSecondaryCpuAbiLegacy() { return state.getSecondaryCpuAbiLegacy(); }

    @Override public String getAppMetadataFilePath() { return state.getAppMetadataFilePath(); }

    @Override public Set<File> getOldPaths() {
        var value = state.getOldPaths();
        return value == null ? null : java.util.Collections.unmodifiableSet(new java.util.LinkedHashSet<>(value));
    }

    @Override public int getAppMetadataSource() { return state.getAppMetadataSource(); }

    @Override public PackageUserStateInternal getUserStateOrDefault(int user) { return users.getOrDefault(user, PackageUserStateInternal.DEFAULT); }
}
