package dev.aim.server;

import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.ActivityInfo;
import android.content.pm.ProviderInfo;
import android.content.pm.SharedLibraryInfo;
import android.content.pm.ProcessInfo;
import android.content.pm.VersionedPackage;
import android.content.pm.PackageInfo;
import android.content.pm.ParceledListSlice;

/** Immutable native query capture. System UID transport; original caller is explicit. */
@JavaPassthrough(annotation="@SuppressWarnings(dev.aim.server.IPackageComputer.GENERIC_RETURN_WARNING)")
interface IPackageComputer {
    const String GENERIC_RETURN_WARNING = "unchecked";
    long getVersion();
    ApplicationInfo getApplicationInfo(String packageName, long flags, int userId,
            int filterCallingUid, int callingUid, int callingPid);
    PackageInfo getPackageInfo(String packageName, long flags, int userId,
            int filterCallingUid, int callingUid, int callingPid);
    boolean filterAppAccess(String packageName, int callingUid, int userId, boolean filterUninstalled);
    void close();
    int getPackageUid(String packageName, long flags, int userId, int callingUid, int callingPid);
    String[] getPackagesForUid(int uid, int callingUid, int callingPid);
    String getNameForUid(int uid, int callingUid, int callingPid);
    boolean isInstantApp(String packageName, int userId, int callingUid, int callingPid);
    int getTargetSdkVersion(String packageName, int callingUid, int callingPid);
    String getInstallerPackageName(String packageName, int userId, int callingUid, int callingPid);
    /** Original PackageManagerInternal UID lookup: SYSTEM_UID filter, without public enforcement. */
    int getPackageUidInternal(String packageName, long flags, int userId);
    /** Original renamed/static-library normalization, before a captured parsed-package lookup. */
    String resolveInternalPackageName(String packageName, long versionCode, int callingUid);
    boolean isSameApp(String packageName, long flags, int comparisonUid, int userId,
            int callingUid, int callingPid);
    boolean filterUidAccess(int targetUid, int callingUid);
    boolean canQueryPackage(int queryUid, String targetPackageName, int callingUid, int callingPid);
    PackageInfo getPackageInfoInternal(String packageName, long versionCode, long flags, int userId,
            int filterCallingUid, int callingUid, int callingPid);
    String getPackageStateFilteredName(String packageName, int callingUid, int userId);
    int getUidTargetSdkVersion(int uid);
    int getUidOwnerRegistryLength();
    byte[] getUidOwnerRegistryChunk(int offset, int length);
    /** Read-only original query protocol, retained version and trusted original caller. */
    IBinder getPackageManagerQueryBinder(int callingUid, int callingPid);
    ApplicationInfo[] getPersistentApplications(boolean safeMode, int flags, int callingUid, int callingPid);
    int getPackageStartability(boolean safeMode, String packageName, int filterCallingUid, int userId,
            int callingUid, int callingPid);
    int getPackageUidWithCaller(String packageName, long flags, int userId, int filterCallingUid,
            int callingUid, int callingPid);
    boolean isCallerSameApp(String packageName, int uid, boolean resolveIsolatedUid,
            int callingUid, int callingPid);
    String getInstantAppPackageName(int uid, int callingUid, int callingPid);
    int getComponentEnabledSetting(in ComponentName component, int filterCallingUid,
            int userId, boolean internal, int callingUid, int callingPid);
    @PropagateAllowBlocking
    ParceledListSlice<ApplicationInfo> getInstalledApplications(long flags, int userId, int filterCallingUid,
            boolean forceAllowCrossUser, int callingUid, int callingPid);
    boolean isInstantAppInternal(String packageName, int userId, int filterCallingUid,
            int callingUid, int callingPid);
    boolean canViewInstantApps(int filterCallingUid, int userId, int callingUid, int callingPid);
    int checkUidSignaturesForAllUsers(int uid1, int uid2, int callingUid, int callingPid);
    void enforceCrossUserPermission(int filterCallingUid, int userId, boolean requireFullPermission,
            boolean checkShell, String message, int callingUid, int callingPid);
    boolean getBlockUninstall(int userId, String packageName);
    SharedLibraryInfo[] getSharedLibraryRegistry();
    boolean shouldFilterApplication(int ownerKind, String ownerName, String packageName, int appId,
            int filterCallingUid, int userId, boolean filterUninstalled, int callingUid, int callingPid);
    ProcessInfo[] getProcessesForUid(int uid, int callingUid, int callingPid);
    int getPackageLookupUid(int uid, boolean knownIsolatedComputeApp, int callingUid, int callingPid);
    String getSetupWizardPackageName();
    VersionedPackage[] getSharedLibraryUsers(String name, long version, int libraryType, long flags,
            int filterCallingUid, int userId, int callingUid, int callingPid);
    boolean[] getSharedLibraryUsersOptional(String name, long version, int libraryType, long flags,
            int filterCallingUid, int userId, int callingUid, int callingPid);
    /** checkOnly returns the null/empty force-queryable marker without user iteration. */
    int[] getVisibilityAllowList(String packageName, int userId, boolean checkOnly);
    ActivityInfo getActivityInfoInternal(in ComponentName component, long flags, int filterCallingUid,
            int userId, int callingUid, int callingPid);
    boolean canAccessComponent(int filterCallingUid, in ComponentName component, int userId,
            int callingUid, int callingPid);
    ProviderInfo resolveContentProvider(String authority, long flags, int userId, int filterCallingUid,
            int callingUid, int callingPid);
    ComponentName getInstantAppInstallerComponent();
    String[] getFrozenPackageNames();
    int[] getFrozenPackageCounts();
    byte[] getInstantAppInstallerInfoRecord();
    boolean activitySupportsIntentAsUser(in ComponentName resolveComponent, in ComponentName component,
            in Intent intent, String resolvedType, int userId, int callingUid, int callingPid);
    boolean hasCrossUserPermission(int filterCallingUid, int userId, boolean requireFullPermission,
            int callingUid, int callingPid);
    byte[] getPlatformSigningDetailsRecord();
    String[] getKnownPackageNames(int kind, int userId, int callingUid, int callingPid);
    boolean isUpgradingFromLowerThan(int sdkVersion);
    String[] getApksInApex(String packageName);
    ComponentName getResolverComponent();
    byte[] queryIntentActivitiesInternalRecord(in Intent intent, String resolvedType, long flags,
            long privateResolveFlags, int filterCallingUid, int filterCallingPid, int userId,
            boolean resolveForStart, boolean allowDynamicSplits, int callingUid, int callingPid);
    byte[] queryIntentServicesInternalRecord(in Intent intent, String resolvedType, long flags, int userId,
            int filterCallingUid, int filterCallingPid, boolean includeInstantApps, boolean resolveForStart,
            int callingUid, int callingPid);
    byte[] resolveIntentInternalRecord(in Intent intent, String resolvedType, long flags, long privateResolveFlags,
            int userId, boolean resolveForStart, int filterCallingUid, int filterCallingPid, int callingUid, int callingPid);
    byte[] resolveServiceInternalRecord(in Intent intent, String resolvedType, long flags, int userId,
            int filterCallingUid, int filterCallingPid, boolean resolveForStart, int callingUid, int callingPid);
    byte[] queryIntentReceiversInternalRecord(in Intent intent, String resolvedType, long flags, int userId,
            int filterCallingUid, int filterCallingPid, boolean forSend, int callingUid, int callingPid);
    boolean isPermissionUpgradeNeeded(int userId);
    boolean hasInstantApplicationMetadata(String packageName, int userId);
    IBinder[] getPreferredRecordTokens(int userId, int kind);
    byte[] getDiagnosticRecord(int kind, int dumpType, String packageName, in String[] permissionNames,
            boolean checkIn, in byte[] dumpState);
    byte[] getLegacyPermissionDefinitionsRecord();
    ActivityInfo getActivityInfoCrossProfile(in ComponentName component, long flags, int userId, int callingUid, int callingPid);
    byte[] getSyncProvidersRecord(boolean safeMode, int callingUid, int callingPid);
    byte[] queryRawComponentsRecord(int kind, in Intent intent, String resolvedType, long flags, String packageName, in ComponentName[] subset, int userId, int callingUid, int callingPid);
    ProviderInfo queryRawProvider(String authority, long flags, int userId);
    byte[] queryRawProvidersRecord(String processName, String metadataKey, int uid, long flags, int userId);
    byte[] queryRawSyncProvidersRecord(boolean safeMode, int userId);
    byte[] dumpRawComponentsRecord(int kind, String packageName, in byte[] dumpState);
    int[] selectPreferredActivity(in Intent intent, String resolvedType, long flags, in ComponentName[] candidates, in int[] matches, boolean always, boolean removeMatches, boolean queryMayBeFiltered, boolean deviceProvisioned, int userId, int callingUid, int callingPid);
    int[] getCrossProfileDomainApproval(in Intent intent, String resolvedType, long flags, int sourceUserId, int parentUserId);
    ActivityInfo getNativeResolverActivity();
    boolean isNativeResolverReplaced();
}
