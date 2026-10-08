package dev.aim.server;
import dev.aim.server.IPackageShellPolicyBridge;
import dev.aim.server.IPackageShellReadLeaf;
import dev.aim.server.IPackageShellInstallPolicy;
import dev.aim.server.IPackageResolutionPolicy;
import dev.aim.server.IPackageDomainSettings;

import android.os.ParcelFileDescriptor;
import android.os.IBinder;
import android.content.pm.ProviderInfo;
import dev.aim.server.IPackageResolverIdentity;
import dev.aim.server.IInstallerConfirmationBridge;
import dev.aim.server.IPackageLifecycleLeaf;
import dev.aim.server.IPackageEnableBridge;
import dev.aim.server.IPackageAppDataBridge;
import dev.aim.server.IInstallerPermissionBridge;
import dev.aim.server.IInstallerPreparationBridge;
import dev.aim.server.IInstallerArchiveQueryBridge;
import dev.aim.server.IInstallerPolicyBridge;
import dev.aim.server.IInstallerRecoveryPresentation;
import dev.aim.server.IPackageInternalStorageBridge;
import dev.aim.server.IPackageApexUninstallBridge;
import dev.aim.server.IPermissionPersistenceBridge;
import dev.aim.server.IPackageDiagnosticInputs;
import dev.aim.server.IPackageInternalEventsBridge;
import dev.aim.server.IPackageShutdownBridge;
import dev.aim.server.IPackageObserverEventsBridge;
import dev.aim.server.IPackageBootContextLeaf;
import dev.aim.server.IPackageInitialContextLeaf;
import dev.aim.server.IPackageBootLifecycleLeaf;
import dev.aim.server.IWebInstantAppsState;
import dev.aim.server.IInstallerCompletionBridge;
import dev.aim.server.IPackageBootConfigurationLeaf;

import android.content.IntentSender;

/** Original owners needed before native PackageManager boot scanning. */

interface IPackageBootstrapBridge {
    IPackageShellPolicyBridge getPackageShellPolicyBridge();
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
    /** Current UserManager shell restriction, independent of DPM startup. */
    boolean isShellDebuggingRestricted(int userId);
    /** Original framework selector used by installer clients in this boot. */
    boolean isInstallerRevocableFdEnabled();
    /** Original storage cache/quota allocation before a native stage write. */
    void allocateInstallerBytes(in ParcelFileDescriptor file, long lengthBytes, int installFlags);
    /** Current original DeviceConfig limits for installer preverified domains. */
    long[] getInstallerDomainLimits();
    /** Current original ART install-file filter selector. */
    boolean isInstallerArtServiceV3Enabled();
    /** Allocate one retained original runtime identity for a native resolver record. */
    IPackageResolverIdentity allocatePreferredResolverIdentity();
    IPackageResolverIdentity allocatePreferredResolverRecord(in byte[] record);
    IInstallerConfirmationBridge getInstallerConfirmationBridge(boolean dependencyInstallerEnabled);
    IPackageLifecycleLeaf getPackageLifecycleLeaf();
    IPackageEnableBridge getPackageEnableBridge();
    IPackageAppDataBridge getPackageAppDataBridge();
    IInstallerPermissionBridge getInstallerPermissionBridge();
    IInstallerPreparationBridge getInstallerPreparationBridge();
    IBinder getInstallerRemovalBridge();
    IInstallerArchiveQueryBridge getInstallerArchiveQueryBridge();
    IBinder getPackageLaunchBridge();
    IInstallerPolicyBridge getInstallerPolicyBridge();
    IInstallerRecoveryPresentation getInstallerRecoveryPresentation();
    IPackageInternalStorageBridge getPackageInternalStorageBridge();
    IPackageApexUninstallBridge getPackageApexUninstallBridge();
    IPermissionPersistenceBridge getPermissionPersistenceBridge();
    IPackageDiagnosticInputs getPackageDiagnosticInputs();
    IPackageInternalEventsBridge getPackageInternalEventsBridge();
    IPackageShutdownBridge getPackageShutdownBridge();
    IPackageObserverEventsBridge getPackageObserverEventsBridge();
    IPackageBootContextLeaf getPackageBootContextLeaf();
    IPackageInitialContextLeaf getPackageInitialContextLeaf();
    IPackageBootLifecycleLeaf getPackageBootLifecycleLeaf();
    IWebInstantAppsState getWebInstantAppsState();
    IInstallerCompletionBridge getInstallerCompletionBridge();
    IPackageBootConfigurationLeaf getPackageBootConfigurationLeaf(boolean factoryTest);
    IBinder getPackageInstallerFiles();
    void invalidatePackagesForUidCache();
    IBinder getNativeStagingBridge();
    IBinder getPackagePolicyBridge();
    boolean isAutoRevokeWhitelisted(int callingUid, String packageName);
    IBinder getInstallerExternalBridge();
    IBinder getPackageMaintenanceBridge();
    IBinder getPackageMutationBridge();
    IBinder getPackageMoveBridge();
    IBinder getPackageRelocationBridge();
    IBinder getApplicationDataBridge();
    boolean checkProviderAuthorityGrants(int callingUid, in ProviderInfo providerInfo, int userId);
    boolean isProviderCloneRedirected(String authority, int callingUid, int userId);
    int getInstantAppCookieLimit();
    int getInstantAppIconDensity();
    int getPackageProfileParent(int userId);
    boolean isParentProfileAppLinkingAllowed(int userId);
    String[] getPackageRoleHolders(String role, int userId);
    int packageMonitorUser(int callingPid, int callingUid, int userId);
    byte[] packageMonitorResult(String action, String packageName, int userId, int uid, boolean replacing);
    boolean waitPackageBackgroundHandler(long timeoutMillis);
    void logPackageProcessStart(String packageName, String processName, int uid,
            String seinfo, String apkFile, int pid);
    /** Permission admission and actual installd/storage preparation after a native existing install. */
    long[] onExistingPackageInstalled(IBinder record, int userId, int installFlags);
    /** Backup owns completion only when it accepts this native restore token. */
    boolean restoreExistingPackageInstall(String packageName, int userId, int token);
    /** Delayed permission restoration and original IntentSender completion. */
    String completeExistingPackageInstall(String packageName, int userId, in IntentSender target, int status, boolean restorePermissions);
    byte[] getExistingPackageInstallUserPolicy(int userId);
    /** Live independent UM policy; native authorization retains the actual caller UID. */
    int getPreferredCrossProfileAccessControl(int sourceUserId, int targetUserId);
    String getPreferredRoleHolder(String role, int userId);
    void setPreferredRoleHolder(String role, String packageName, int userId, boolean broadcastOnSuccess);
    void sendPreferredActivityChanged(int userId);
    void resetPreferredRuntimePermissions(int userId);
    void resetPreferredNetworkPolicies(int userId);
    IPackageResolutionPolicy getPackageResolutionPolicy();
    IPackageShellReadLeaf getPackageShellReadLeaf();
    IPackageShellInstallPolicy getPackageShellInstallPolicy();
    IPackageDomainSettings getPackageDomainSettings();
    /** Original guest timezone and Settings diagnostic date formatting. */
    String formatPackageTimestamp(long millis);
    /** Current SDK library dependency policy from the original framework flags. */
    boolean isSdkLibraryIndependenceEnabled();
}
