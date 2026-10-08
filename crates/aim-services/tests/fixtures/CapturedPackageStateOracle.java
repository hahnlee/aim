final class CapturedPackageStateOracle {
    static void verify(dev.aim.server.PackageStateReplica replica, com.android.server.pm.PackageSetting original) {
        var expected = (com.android.server.pm.pkg.PackageStateInternal) original;
        var firstSigning = replica.getSigningDetails();
        var expectedSigning = expected.getSigningDetails();
        if (!java.util.Arrays.equals(firstSigning.getSignatures(), expectedSigning.getSignatures())
                || !java.util.Objects.equals(firstSigning.getPublicKeys(), expectedSigning.getPublicKeys()))
            throw new AssertionError("retained saved signer differs");
        var firstKeys = firstSigning.getPublicKeys();
        if (firstKeys != null) {
            firstKeys.clear();
            if (!java.util.Objects.equals(replica.getSigningDetails().getPublicKeys(), expectedSigning.getPublicKeys()))
                throw new AssertionError("retained signer public keys are shared");
        }
        var firstSignatures = firstSigning.getSignatures();
        if (firstSignatures != null && firstSignatures.length != 0) {
            int flags = replica.getSigningDetails().getSignatures()[0].getFlags();
            firstSignatures[0].setFlags(flags ^ 31);
            firstSignatures[0] = null;
            if (replica.getSigningDetails().getSignatures()[0].getFlags() != flags)
                throw new AssertionError("retained current signer is shared");
        }
        var firstPast = firstSigning.getPastSigningCertificates();
        if (firstPast != null && firstPast.length != 0) {
            int flags = replica.getSigningDetails().getPastSigningCertificates()[0].getFlags();
            firstPast[0].setFlags(flags ^ 31);
            firstPast[0] = null;
            if (replica.getSigningDetails().getPastSigningCertificates()[0].getFlags() != flags)
                throw new AssertionError("retained signer lineage is shared");
        }

        if (!java.util.Objects.deepEquals(replica.getLastPackageUsageTime(), expected.getLastPackageUsageTime())) throw new AssertionError("captured getLastPackageUsageTime differs");
        if (!java.util.Objects.deepEquals(replica.getApexModuleName(), expected.getApexModuleName())) throw new AssertionError("captured getApexModuleName differs");
        if (!java.util.Objects.deepEquals(replica.getAppId(), expected.getAppId())) throw new AssertionError("captured getAppId differs");
        if (!java.util.Objects.deepEquals(replica.getCategoryOverride(), expected.getCategoryOverride())) throw new AssertionError("captured getCategoryOverride differs");
        if (!java.util.Objects.deepEquals(replica.getCpuAbiOverride(), expected.getCpuAbiOverride())) throw new AssertionError("captured getCpuAbiOverride differs");
        if (!java.util.Objects.deepEquals(replica.getHiddenApiEnforcementPolicy(), expected.getHiddenApiEnforcementPolicy())) throw new AssertionError("captured getHiddenApiEnforcementPolicy differs");
        if (!java.util.Objects.deepEquals(replica.getLastModifiedTime(), expected.getLastModifiedTime())) throw new AssertionError("captured getLastModifiedTime differs");
        if (!java.util.Objects.deepEquals(replica.getLastUpdateTime(), expected.getLastUpdateTime())) throw new AssertionError("captured getLastUpdateTime differs");
        if (!java.util.Objects.deepEquals(replica.getMimeGroups(), expected.getMimeGroups())) throw new AssertionError("captured getMimeGroups differs");
        if (!java.util.Objects.deepEquals(replica.getPackageName(), expected.getPackageName())) throw new AssertionError("captured getPackageName differs");
        if (!java.util.Objects.deepEquals(replica.getPath(), expected.getPath())) throw new AssertionError("captured getPath differs");
        if (!java.util.Objects.deepEquals(replica.getPrimaryCpuAbi(), expected.getPrimaryCpuAbi())) throw new AssertionError("captured getPrimaryCpuAbi differs");
        if (!java.util.Objects.deepEquals(replica.getRestrictUpdateHash(), expected.getRestrictUpdateHash())) throw new AssertionError("captured getRestrictUpdateHash differs");
        if (!java.util.Objects.deepEquals(replica.getSeInfo(), expected.getSeInfo())) throw new AssertionError("captured getSeInfo differs");
        if (!java.util.Objects.deepEquals(replica.getSecondaryCpuAbi(), expected.getSecondaryCpuAbi())) throw new AssertionError("captured getSecondaryCpuAbi differs");
        if (!java.util.Objects.deepEquals(replica.getSharedUserAppId(), expected.getSharedUserAppId())) throw new AssertionError("captured getSharedUserAppId differs");
        if (!java.util.Objects.deepEquals(replica.getTargetSdkVersion(), expected.getTargetSdkVersion())) throw new AssertionError("captured getTargetSdkVersion differs");
        if (!java.util.Objects.deepEquals(replica.getUsesLibraryFiles(), expected.getUsesLibraryFiles())) throw new AssertionError("captured getUsesLibraryFiles differs");
        if (!java.util.Objects.deepEquals(replica.getUsesSdkLibraries(), expected.getUsesSdkLibraries())) throw new AssertionError("captured getUsesSdkLibraries differs");
        if (!java.util.Objects.deepEquals(replica.getUsesSdkLibrariesOptional(), expected.getUsesSdkLibrariesOptional())) throw new AssertionError("captured getUsesSdkLibrariesOptional differs");
        if (!java.util.Objects.deepEquals(replica.getUsesSdkLibrariesVersionsMajor(), expected.getUsesSdkLibrariesVersionsMajor())) throw new AssertionError("captured getUsesSdkLibrariesVersionsMajor differs");
        if (!java.util.Objects.deepEquals(replica.getUsesStaticLibraries(), expected.getUsesStaticLibraries())) throw new AssertionError("captured getUsesStaticLibraries differs");
        if (!java.util.Objects.deepEquals(replica.getUsesStaticLibrariesVersions(), expected.getUsesStaticLibrariesVersions())) throw new AssertionError("captured getUsesStaticLibrariesVersions differs");
        if (!java.util.Objects.deepEquals(replica.getVersionCode(), expected.getVersionCode())) throw new AssertionError("captured getVersionCode differs");
        if (!java.util.Objects.deepEquals(replica.getVolumeUuid(), expected.getVolumeUuid())) throw new AssertionError("captured getVolumeUuid differs");
        if (!java.util.Objects.deepEquals(replica.hasSharedUser(), expected.hasSharedUser())) throw new AssertionError("captured hasSharedUser differs");
        if (!java.util.Objects.deepEquals(replica.isApex(), expected.isApex())) throw new AssertionError("captured isApex differs");
        if (!java.util.Objects.deepEquals(replica.isApkInUpdatedApex(), expected.isApkInUpdatedApex())) throw new AssertionError("captured isApkInUpdatedApex differs");
        if (!java.util.Objects.deepEquals(replica.isDebuggable(), expected.isDebuggable())) throw new AssertionError("captured isDebuggable differs");
        if (!java.util.Objects.deepEquals(replica.isDefaultToDeviceProtectedStorage(), expected.isDefaultToDeviceProtectedStorage())) throw new AssertionError("captured isDefaultToDeviceProtectedStorage differs");
        if (!java.util.Objects.deepEquals(replica.isExternalStorage(), expected.isExternalStorage())) throw new AssertionError("captured isExternalStorage differs");
        if (!java.util.Objects.deepEquals(replica.isForceQueryableOverride(), expected.isForceQueryableOverride())) throw new AssertionError("captured isForceQueryableOverride differs");
        if (!java.util.Objects.deepEquals(replica.isHiddenUntilInstalled(), expected.isHiddenUntilInstalled())) throw new AssertionError("captured isHiddenUntilInstalled differs");
        if (!java.util.Objects.deepEquals(replica.isInstallPermissionsFixed(), expected.isInstallPermissionsFixed())) throw new AssertionError("captured isInstallPermissionsFixed differs");
        if (!java.util.Objects.deepEquals(replica.isLeavingSharedUser(), expected.isLeavingSharedUser())) throw new AssertionError("captured isLeavingSharedUser differs");
        if (!java.util.Objects.deepEquals(replica.isOdm(), expected.isOdm())) throw new AssertionError("captured isOdm differs");
        if (!java.util.Objects.deepEquals(replica.isOem(), expected.isOem())) throw new AssertionError("captured isOem differs");
        if (!java.util.Objects.deepEquals(replica.isPageSizeAppCompatEnabled(), expected.isPageSizeAppCompatEnabled())) throw new AssertionError("captured isPageSizeAppCompatEnabled differs");
        if (!java.util.Objects.deepEquals(replica.isPendingRestore(), expected.isPendingRestore())) throw new AssertionError("captured isPendingRestore differs");
        if (!java.util.Objects.deepEquals(replica.isPersistent(), expected.isPersistent())) throw new AssertionError("captured isPersistent differs");
        if (!java.util.Objects.deepEquals(replica.isPrivileged(), expected.isPrivileged())) throw new AssertionError("captured isPrivileged differs");
        if (!java.util.Objects.deepEquals(replica.isProduct(), expected.isProduct())) throw new AssertionError("captured isProduct differs");
        if (!java.util.Objects.deepEquals(replica.isRequiredForSystemUser(), expected.isRequiredForSystemUser())) throw new AssertionError("captured isRequiredForSystemUser differs");
        if (!java.util.Objects.deepEquals(replica.isScannedAsStoppedSystemApp(), expected.isScannedAsStoppedSystemApp())) throw new AssertionError("captured isScannedAsStoppedSystemApp differs");
        if (!java.util.Objects.deepEquals(replica.isSystem(), expected.isSystem())) throw new AssertionError("captured isSystem differs");
        if (!java.util.Objects.deepEquals(replica.isSystemExt(), expected.isSystemExt())) throw new AssertionError("captured isSystemExt differs");
        if (!java.util.Objects.deepEquals(replica.isUpdateAvailable(), expected.isUpdateAvailable())) throw new AssertionError("captured isUpdateAvailable differs");
        if (!java.util.Objects.deepEquals(replica.isUpdatedSystemApp(), expected.isUpdatedSystemApp())) throw new AssertionError("captured isUpdatedSystemApp differs");
        if (!java.util.Objects.deepEquals(replica.isVendor(), expected.isVendor())) throw new AssertionError("captured isVendor differs");
        if (!java.util.Objects.deepEquals(replica.getDomainSetId(), expected.getDomainSetId())) throw new AssertionError("captured getDomainSetId differs");
        if (!java.util.Objects.deepEquals(replica.getFlags(), expected.getFlags())) throw new AssertionError("captured getFlags differs");
        if (!java.util.Objects.deepEquals(replica.getPrivateFlags(), expected.getPrivateFlags())) throw new AssertionError("captured getPrivateFlags differs");
        if (!java.util.Objects.deepEquals(replica.getRealName(), expected.getRealName())) throw new AssertionError("captured getRealName differs");
        if (!java.util.Objects.deepEquals(replica.isLoading(), expected.isLoading())) throw new AssertionError("captured isLoading differs");
        if (!java.util.Objects.deepEquals(replica.getPathString(), expected.getPathString())) throw new AssertionError("captured getPathString differs");
        if (!java.util.Objects.deepEquals(replica.getLoadingProgress(), expected.getLoadingProgress())) throw new AssertionError("captured getLoadingProgress differs");
        if (!java.util.Objects.deepEquals(replica.getLoadingCompletedTime(), expected.getLoadingCompletedTime())) throw new AssertionError("captured getLoadingCompletedTime differs");
        if (!java.util.Objects.deepEquals(replica.getPrimaryCpuAbiLegacy(), expected.getPrimaryCpuAbiLegacy())) throw new AssertionError("captured getPrimaryCpuAbiLegacy differs");
        if (!java.util.Objects.deepEquals(replica.getSecondaryCpuAbiLegacy(), expected.getSecondaryCpuAbiLegacy())) throw new AssertionError("captured getSecondaryCpuAbiLegacy differs");
        if (!java.util.Objects.deepEquals(replica.getAppMetadataFilePath(), expected.getAppMetadataFilePath())) throw new AssertionError("captured getAppMetadataFilePath differs");
        if (!java.util.Objects.deepEquals(replica.getOldPaths(), expected.getOldPaths())) throw new AssertionError("captured getOldPaths differs");
        if (!java.util.Objects.deepEquals(replica.getAppMetadataSource(), expected.getAppMetadataSource())) throw new AssertionError("captured getAppMetadataSource differs");
        if (replica.getPkg() != replica.getAndroidPackage()
                || replica.getPkg() != replica.getPkg()) throw new AssertionError("captured parsed object identity differs");
        com.android.server.pm.CapturedKeySetOracle.verifyReplica(replica, expected);
        if (!replica.getSigningDetails().equals(expected.getSigningDetails())
                || !replica.getSigningInfo().getSigningDetails().equals(expected.getSigningInfo().getSigningDetails())) throw new AssertionError("replica signing differs");
        for (int user : new int[] {0, 10}) {
            var actual = replica.getLegacyPermissionState();
            var permissions = expected.getLegacyPermissionState();
            if (actual.isMissing(user) != permissions.isMissing(user)
                    || actual.getPermissionStates(user).size() != permissions.getPermissionStates(user).size()) throw new AssertionError("replica permissions differ");
            for (var permission : permissions.getPermissionStates(user)) {
                var captured = actual.getPermissionState(permission.getName(), user);
                if (captured == null || captured.isRuntime() != permission.isRuntime()
                        || captured.isGranted() != permission.isGranted() || captured.getFlags() != permission.getFlags()) throw new AssertionError("replica permission fields differ");
            }
            actual.reset();
            if (replica.getLegacyPermissionState().isMissing(user) != permissions.isMissing(user)
                    || replica.getLegacyPermissionState().getPermissionStates(user).size() != permissions.getPermissionStates(user).size()) throw new AssertionError("mutable replica permissions escaped");
        }
        var users = replica.getUserStates();
        if (users.size() != expected.getUserStates().size()) throw new AssertionError("captured user inventory differs");
        for (int i = 0; i < users.size(); i++) {
            int id = users.keyAt(i);
            if (id != expected.getUserStates().keyAt(i)
                    || users.valueAt(i) != replica.getUserStateOrDefault(id)
                    || users.valueAt(i) != replica.getStateForUser(android.os.UserHandle.of(id))) throw new AssertionError("captured user identity differs");
        }
        if (replica.getUserStateOrDefault(999) != com.android.server.pm.pkg.PackageUserStateInternal.DEFAULT) throw new AssertionError("missing user default differs");
        users.put(999, null);
        if (replica.getUserStates().size() != expected.getUserStates().size()) throw new AssertionError("mutable sparse users escaped");
        long firstUsage = expected.getLastPackageUsageTime()[0];
        var usage = replica.getLastPackageUsageTime(); usage[0] = 999;
        replica.getTransientState().setLastPackageUsageTimeInMills(0, 999);
        if (replica.getLastPackageUsageTime()[0] != firstUsage) throw new AssertionError("mutable usage escaped capture");
        var hash = replica.getRestrictUpdateHash();
        if (hash != null && hash.length != 0) {
            byte first = hash[0]; hash[0]++;
            if (replica.getRestrictUpdateHash()[0] != first) throw new AssertionError("mutable update hash escaped capture");
        }
        try { replica.getMimeGroups().clear(); throw new AssertionError("mutable MIME map escaped"); } catch (UnsupportedOperationException expectedError) {}
        for (var types : replica.getMimeGroups().values()) {
            try { types.clear(); throw new AssertionError("mutable MIME types escaped"); } catch (UnsupportedOperationException expectedError) {}
        }
        replica.getTransientState().setHiddenUntilInstalled(!replica.isHiddenUntilInstalled());
        if (replica.isHiddenUntilInstalled() != expected.isHiddenUntilInstalled()) throw new AssertionError("mutable transient state escaped capture");
    }
}
