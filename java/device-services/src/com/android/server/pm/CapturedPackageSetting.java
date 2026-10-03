package com.android.server.pm;

import dev.aim.server.PackageSettingData;
import dev.aim.server.PackageUserStateData;
import dev.aim.server.PackageUserStateReplica;
import java.io.File;
import java.util.List;
import java.util.UUID;

/** Original concrete metadata and sparse user owners; code/transient owners attach separately. */
public final class CapturedPackageSetting {
    private CapturedPackageSetting() {}

    public static PackageSetting withUsers(PackageSettingData data, List<PackageUserStateData> users,
            long version, boolean factory, boolean crossUserSuspensions) {
        int previous = -1;
        for (var user : users) {
            if (user.getVersion() != version || user.isFactory() != factory
                    || !user.getPackageName().equals(data.getPackageName()) || user.getAppId() != data.appId
                    || user.getUserId() <= previous) throw new IllegalArgumentException("package user capture mismatch");
            previous = user.getUserId();
        }
        var setting = from(data, version, factory);
        for (var user : users) {
            var source = new PackageUserStateReplica(user, crossUserSuspensions);
            var target = setting.getOrCreateUserState(user.getUserId());
            target.setCeDataInode(user.ceDataInode).setDeDataInode(user.deDataInode)
                .setEnabledState(user.enabled).setInstalled(user.installed).setStopped(user.stopped)
                .setNotLaunched(user.notLaunched).setHidden(user.hidden).setDistractionFlags(user.distractionFlags)
                .setInstantApp(user.instantApp).setVirtualPreload(user.virtualPreload)
                .setLastDisableAppCaller(user.lastDisableCaller).setInstallReason(user.installReason)
                .setUninstallReason(user.uninstallReason).setHarmfulAppWarning(user.harmfulWarning)
                .setSplashScreenTheme(user.splashTheme).setFirstInstallTimeMillis(user.firstInstallTime)
                .setMinAspectRatio(user.minAspectRatio).setArchiveState(source.getArchiveState());
            // Null components retain the fresh original unallocated owners.
            if (user.getEnabledComponents() != null) target.setEnabledComponents(source.getEnabledComponents());
            if (user.getDisabledComponents() != null) target.setDisabledComponents(source.getDisabledComponents());
            var suspensions = source.getSuspendParams();
            if (suspensions != null) target.setSuspendParams(suspensions.untrackedStorage());
            if (user.overlayPaths != null) {
                target.setOverlayPaths(source.getOverlayPaths());
                if (target.getOverlayPaths() == null) throw new IllegalArgumentException("captured overlay cannot be reproduced");
            }
            if (user.getLibraryOverlays() != null) {
                var overlays = source.getSharedLibraryOverlayPaths();
                // Removing an absent library allocates the original empty map.
                if (overlays.isEmpty()) target.setSharedLibraryOverlayPaths("", null);
                for (var entry : overlays.entrySet()) target.setSharedLibraryOverlayPaths(entry.getKey(), entry.getValue());
                if (!new java.util.LinkedHashMap<>(target.getSharedLibraryOverlayPaths()).equals(overlays)) throw new IllegalArgumentException("captured library overlay cannot be reproduced");
            }
            if (user.getLabelIcons() != null) {
                if (user.getLabelIcons().isEmpty()) throw new IllegalArgumentException("allocated-empty label owner cannot be reproduced");
                for (var label : user.getLabelIcons()) {
                    var component = new android.content.ComponentName(label.packageName, label.className);
                    target.overrideLabelAndIcon(component, label.label, label.icon);
                    if (!java.util.Objects.equals(target.getOverrideLabelIconForComponent(component),
                            new android.util.Pair<>(label.label, label.icon))) throw new IllegalArgumentException("captured label cannot be reproduced");
                }
            }
        }
        return setting;
    }

    public static PackageSetting from(PackageSettingData data, long version, boolean factory) {
        if (data.getVersion() != version || data.isFactory() != factory) {
            throw new IllegalArgumentException("package setting capture mismatch");
        }
        UUID domain = data.domainSetId != null ? UUID.fromString(data.domainSetId)
            : factory ? com.android.server.pm.verify.domain.DomainVerificationManagerInternal.DISABLED_ID : null;
        if (domain == null) throw new IllegalStateException("active domain owner is not assigned");
        // Resolve permission inputs before constructing any original owner.
        boolean fixed = data.isInstallPermissionsFixed();
        boolean leaving = data.isLeavingSharedUser();
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
        setting.setLeavingSharedUser(leaving);
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
