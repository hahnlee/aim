package dev.aim.server;

/** Original owners needed before native PackageManager boot scanning. */
interface IPackageBootstrapBridge {
    boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk);
    boolean isTestBaseOnBootclasspath();
    int[] getPermissionGidsForUid(int uid);
    /** Non-shared SELinuxMMAC compatibility decision for original parsed code. */
    int getSeInfoTargetSdkVersion(in byte[] packageCache);
    /** Projection of the original permission owner over resolved users, including pre-created users. */
    byte[] getLegacyPermissionState(int appId, in int[] userIds);
    /** UUID from the original domain owner, most-significant word first. */
    byte[] generateNewDomainId();
    /** Original resolved scan users, including pre-created users and ADB restrictions. */
    byte[] getPackageScanUsers();
    /** Original all-package and active-mount APEX inventories, preserving owner order. */
    byte[] getApexBootInventory();
    /** Completed native containers delivered to original ApexManager before APK scanning. */
    void notifyApexScanResults(in byte[] scanResults);
    /** Original SharedUidMigration.applyStrategy(BEST_EFFORT) decision. */
    boolean isSharedUidMigrationBestEffort();
    /** Original AndroidTestBaseUpdater change, using full parsed ApplicationInfo. */
    boolean isTestBaseLibraryChangeEnabled(in byte[] packageCache);
    /** Pinned Build policy for native signing test APIs. */
    boolean isSigningDebuggable();
    /** AppsFilter FeatureConfig decision from original PlatformCompat. */
    boolean isApplicationQueryFilteringEnabled(String packageName, int targetSdk);
    /** Installed definitions from the original permission front end. */
    String[] getPackageInstalledPermissions(String packageName);
    /** Grants from the exact expected current PackageManagerLocal UID owner. */
    String[] getPackageGrantedPermissions(String packageName, int appId, int userId);
    /** Domain collector's RESTRICT_DOMAINS decision from original PlatformCompat. */
    boolean isDomainVerificationRestricted(String packageName, int targetSdk);
    /** Original package-info cache invalidation after a committed native change. */
    void invalidatePackageInfoCache();
    /** Current original domain proxy identity check; never a fixed native UID. */
    boolean isDomainVerifierUid(int uid);
    /** Domain approval legacy/V2 decision from original PlatformCompat. */
    boolean isDomainVerificationSettingsV2Enabled(String packageName, int targetSdk);
    /** UUID.fromString mode of the original service process. */
    boolean isDomainSetUuidStrictValidationEnabled();
    /** Current original Settings.VersionInfo.forceCurrent values, parcel-encoded. */
    byte[] getCurrentPackageVersion();
    /** Original libselinux restores the native installer's created file/stage label. */
    void restoreInstallerContext(String path);
    /** Current original user/DPM owners: user ID, exists, install/debug restrictions, managed. */
    byte[] getInstallerUserPolicy(int userId);
}
