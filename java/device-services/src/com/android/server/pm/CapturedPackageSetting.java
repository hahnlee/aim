package com.android.server.pm;

import dev.aim.server.PackageSettingData;
import java.io.File;
import java.util.UUID;

/** Original concrete metadata owner; code, users and transient owners attach separately. */
public final class CapturedPackageSetting {
    private CapturedPackageSetting() {}

    public static PackageSetting from(PackageSettingData data, long version, boolean factory) {
        if (data.getVersion() != version || data.isFactory() != factory) {
            throw new IllegalArgumentException("package setting capture mismatch");
        }
        UUID domain = data.domainSetId != null ? UUID.fromString(data.domainSetId)
            : factory ? com.android.server.pm.verify.domain.DomainVerificationManagerInternal.DISABLED_ID : null;
        if (domain == null) throw new IllegalStateException("active domain owner is not assigned");
        // Resolve permission inputs before constructing any original owner.
        boolean fixed = data.isInstallPermissionsFixed();
        var legacy = data.getLegacyPermissionState();
        var setting = new PackageSetting(data.getPackageName(), data.realName,
            new File(data.path), data.flags, data.privateFlags, domain);
        setting.setAppId(data.appId);
        if (data.sharedUser) setting.setSharedUserAppId(data.appId);
        setting.setLegacyNativeLibraryPath(data.legacyNativeLibraryPath);
        setting.setPrimaryCpuAbi(data.primaryCpuAbiRaw);
        setting.setSecondaryCpuAbi(data.secondaryCpuAbiRaw);
        setting.setCpuAbiOverride(data.cpuAbiOverride);
        setting.setLastModifiedTime(data.lastModifiedTime);
        setting.setLastUpdateTime(data.lastUpdateTime);
        setting.setLongVersionCode(data.versionCode);
        setting.setTargetSdkVersion(data.targetSdkVersion);
        setting.setRestrictUpdateHash(data.getRestrictUpdateHash());
        setting.setVolumeUuid(data.volumeUuid);
        setting.setCategoryOverride(data.categoryOverride);
        setting.setUpdateAvailable(data.updateAvailable);
        setting.setForceQueryableOverride(data.forceQueryable);
        setting.setPendingRestore(data.pendingRestore);
        setting.setDebuggable(data.debuggable);
        setting.setScannedAsStoppedSystemApp(data.scannedAsStoppedSystemApp);
        setting.setBaseRevisionCode(data.baseRevisionCode);
        setting.setPageSizeAppCompatFlags(data.pageSizeAppCompatFlags);
        setting.setLoadingProgress(data.loadingProgress);
        setting.setLoadingCompletedTime(data.loadingCompletedTime);
        setting.setAppMetadataFilePath(data.appMetadataFilePath);
        setting.setAppMetadataSource(data.appMetadataSource);
        setting.setInstallSource(data.getInstallSource());
        CapturedKeySetData.populate(data.keySets, setting.getKeySetData());
        setting.setUsesSdkLibraries(data.getUsesSdkLibraries());
        setting.setUsesSdkLibrariesVersionsMajor(data.getUsesSdkLibrariesVersionsMajor());
        setting.setUsesSdkLibrariesOptional(data.getUsesSdkLibrariesOptional());
        setting.setUsesStaticLibraries(data.getUsesStaticLibraries());
        setting.setUsesStaticLibrariesVersions(data.getUsesStaticLibrariesVersions());
        for (var group : data.getMimeGroups().entrySet()) setting.addMimeTypes(group.getKey(), group.getValue());
        var paths = data.getOldPaths();
        if (paths != null) {
            if (paths.isEmpty()) {
                // Original mutations allocate and retain an empty LinkedHashSet.
                var temporary = new File("");
                setting.addOldPath(temporary).removeOldPath(temporary);
            } else for (String path : paths) setting.addOldPath(path == null ? null : new File(path));
        }
        setting.getLegacyPermissionState().copyFrom(legacy);
        setting.setInstallPermissionsFixed(fixed);
        if (setting.getPageSizeAppCompatFlags() != data.pageSizeAppCompatFlags
                || Float.floatToIntBits(setting.getLoadingProgress()) != Float.floatToIntBits(data.loadingProgress)) {
            throw new IllegalArgumentException("captured setter state is not reproducible");
        }
        return setting;
    }
}
