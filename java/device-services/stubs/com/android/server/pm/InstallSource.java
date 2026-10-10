// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm;
public final class InstallSource {
    static final InstallSource EMPTY = null;
    final String mInitiatingPackageName, mOriginatingPackageName, mInstallerPackageName,
        mUpdateOwnerPackageName, mInstallerAttributionTag;
    final int mInstallerPackageUid, mPackageSource;
    final boolean mIsOrphaned, mIsInitiatingPackageUninstalled;
    final PackageSignatures mInitiatingPackageSignatures;
    InstallSource setInitiatingPackageSignatures(PackageSignatures signatures) { throw new RuntimeException("stub"); }
    private InstallSource(String initiating, String originating, String installer, int uid,
        String updateOwner, String tag, boolean orphaned, boolean uninstalled,
        PackageSignatures signatures, int source) { throw new RuntimeException("stub"); }
    static InstallSource create(String initiating, String originating, String installer, int uid,
        String updateOwner, String tag, int source, boolean orphaned, boolean uninstalled) {
        throw new RuntimeException("stub");
    }
}
