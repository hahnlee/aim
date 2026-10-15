package com.android.server.pm;

import dev.aim.server.PackageSettingData;

/** Rebuild original package-private factory inputs from one immutable capture. */
public final class CapturedInstallSource {
    private CapturedInstallSource() {}
    public static InstallSource from(PackageSettingData.InstallSourceData data) {
        InstallSource source = InstallSource.create(data.initiatingPackage, data.originatingPackage,
            data.installerPackage, data.installerUid, data.updateOwner, data.attributionTag,
            data.packageSource, data.orphaned, data.initiatingUninstalled);
        var signing = data.getInitiatingSigningDetails();
        if (signing == null) return source;
        var signatures = new PackageSignatures();
        signatures.mSigningDetails = signing;
        return source.setInitiatingPackageSignatures(signatures);
    }
}
