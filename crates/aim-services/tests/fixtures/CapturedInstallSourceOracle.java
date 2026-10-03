package com.android.server.pm;

public final class CapturedInstallSourceOracle {
    public static void verify(dev.aim.server.PackageSettingData data, android.content.pm.SigningDetails expected, boolean populated) {
        InstallSource source = data.getInstallSource();
        if (InstallSource.create(null, null, null, 123, null, "discarded", 0, false, false) != InstallSource.EMPTY
                || InstallSource.create(null, null, null, 123, null, "discarded", 0, true, false) != InstallSource.create(null, null, null, -1, null, null, 0, true, false)) throw new AssertionError("original empty owner normalization changed");
        var retained = InstallSource.create(null, null, null, 123, null, "tag", 3, false, false);
        if (retained.mInstallerPackageUid != 123 || retained.mPackageSource != 3 || !retained.mInstallerAttributionTag.equals("tag")) throw new AssertionError("nondefault source was coalesced");
        try { InstallSource.EMPTY.setInitiatingPackageSignatures(new PackageSignatures()); throw new AssertionError("original invalid signing owner accepted"); } catch (IllegalArgumentException expectedError) {}

        if (!populated) {
            if (source != (data.installSource.orphaned ? InstallSource.create(null, null, null, -1, null, null, 0, true, false) : InstallSource.EMPTY) || data.installSource.getInitiatingSigningDetails() != null) throw new AssertionError("default install source differs");
            return;
        }
        if (!source.mInitiatingPackageName.equals(data.getPackageName()) || !source.mOriginatingPackageName.equals("origin")
                || !source.mInstallerPackageName.equals("") || source.mInstallerPackageUid != 123
                || !source.mUpdateOwnerPackageName.equals("owner") || !source.mInstallerAttributionTag.equals("tag")
                || source.mPackageSource != 3 || !source.mIsOrphaned || !source.mIsInitiatingPackageUninstalled
                || source.mInitiatingPackageSignatures == null || !source.mInitiatingPackageSignatures.mSigningDetails.equals(expected)) {
            throw new AssertionError("original install source fields/signing differ");
        }
        source.mInitiatingPackageSignatures.mSigningDetails.getSignatures()[0] = null;
        if (!data.getInstallSource().mInitiatingPackageSignatures.mSigningDetails.equals(expected)) throw new AssertionError("install source signing mutation escaped capture");
    }
}
