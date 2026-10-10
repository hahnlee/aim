// Compile-only pinned image API, android-16.0.0_r1; checked by image linkage.
package android.content.pm;
import android.content.ComponentName;
import android.content.Intent;
import android.content.IntentSender;
import android.content.pm.overlay.OverlayPaths;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.util.ArrayMap;
import android.util.ArraySet;
import android.util.SparseArray;
import com.android.internal.pm.pkg.component.ParsedMainComponent;
import com.android.server.pm.PackageArchiver;
import com.android.server.pm.PackageList;
import com.android.server.pm.PackageSetting;
import com.android.server.pm.dex.DynamicCodeLogger;
import com.android.server.pm.permission.LegacyPermissionSettings;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.SharedUserApi;
import com.android.server.pm.pkg.mutate.PackageStateMutator;
import com.android.server.pm.snapshot.PackageDataSnapshot;
import java.io.IOException;
import java.util.List;
import java.util.Set;
import java.util.concurrent.Executor;
import java.util.function.Consumer;
public abstract class PackageManagerInternal {
public interface PackageListObserver {
    public default void onPackageAdded(String packageName, int uid) { throw new RuntimeException("stub"); }
    public default void onPackageChanged(String packageName, int uid) { throw new RuntimeException("stub"); }
    public default void onPackageRemoved(String packageName, int uid) { throw new RuntimeException("stub"); }
}
    public abstract void setKeepUninstalledPackages(List<String> packageList);
    public abstract boolean isPermissionsReviewRequired(String packageName, int userId);
    public abstract boolean isSameApp(String packageName, int callingUid, int userId);
    public abstract boolean isSameApp(String packageName, long flags, int callingUid, int userId);
    public abstract PackageInfo getPackageInfo(String packageName, long flags, int filterCallingUid, int userId);
    public abstract List<ApplicationInfo> getInstalledApplications(long flags, int userId, int callingUid);
    public abstract List<ApplicationInfo> getInstalledApplicationsCrossUser(long flags, int userId, int callingUid);
    public abstract Bundle getSuspendedPackageLauncherExtras(String packageName, int userId);
    public abstract boolean isPackageSuspended(String packageName, int userId);
    public abstract void removeAllNonSystemPackageSuspensions(int userId);
    public abstract void removeNonSystemPackageSuspensions(String packageName, int userId);
    public abstract void removeDistractingPackageRestrictions(String packageName, int userId);
    public abstract void removeAllDistractingPackageRestrictions(int userId);
    public abstract void flushPackageRestrictions(int userId);
    public abstract UserPackage getSuspendingPackage(String suspendedPackage, int userId);
    public abstract String[] setPackagesSuspendedByAdmin(int userId, String[] packageNames, boolean suspended);
    public abstract SuspendDialogInfo getSuspendedDialogInfo(String suspendedPackage, UserPackage suspendingPackage, int userId);
    public abstract int getDistractingPackageRestrictions(String packageName, int userId);
    public abstract int getPackageUid(String packageName, long flags, int userId);
    public abstract ApplicationInfo getApplicationInfo(String packageName, long flags, int filterCallingUid, int userId);
    public abstract ActivityInfo getActivityInfo(ComponentName component, long flags, int filterCallingUid, int userId);
    public abstract List<ResolveInfo> queryIntentActivities(Intent intent, String resolvedType, long flags, int filterCallingUid, int userId);
    public abstract List<ResolveInfo> queryIntentReceivers(Intent intent, String resolvedType, long flags, int filterCallingUid, int callingPid, int userId, boolean forSend);
    public abstract List<ResolveInfo> queryIntentServices(Intent intent, long flags, int callingUid, int userId);
    public abstract ComponentName getHomeActivitiesAsUser(List<ResolveInfo> allHomeCandidates, int userId);
    public abstract ComponentName getDefaultHomeActivity(int userId);
    public abstract ComponentName getSystemUiServiceComponent();
    public abstract void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwner, SparseArray<String> profileOwners);
    public abstract void setOwnerProtectedPackages(int userId, List<String> packageNames);
    public abstract boolean isPackageDataProtected(int userId, String packageName);
    public abstract boolean isPackageStateProtected(String packageName, int userId);
    public abstract boolean isPackageEphemeral(int userId, String packageName);
    public abstract boolean wasPackageEverLaunched(String packageName, int userId);
    public abstract String getNameForUid(int uid);
    public abstract void requestInstantAppResolutionPhaseTwo(AuxiliaryResolveInfo responseObj, Intent origIntent, String resolvedType, String callingPkg, String callingFeatureId, boolean isRequesterInstantApp, Bundle verificationBundle, int userId);
    public abstract void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct);
    public abstract void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct, boolean retainOnUpdate);
    public abstract boolean isInstantAppInstallerComponent(ComponentName component);
    public abstract void pruneInstantApps();
    public abstract String getSetupWizardPackageName();
public interface ExternalSourcesPolicy {



    public int getPackageTrustedToInstallApps(String packageName, int uid);
}
    public abstract void setExternalSourcesPolicy(ExternalSourcesPolicy policy);
    public abstract boolean isPackagePersistent(String packageName);
    public abstract List<String> getTargetPackageNames(int userId);
    public abstract void setEnabledOverlayPackages(int userId, ArrayMap<String, OverlayPaths> pendingChanges, Set<String> outUpdatedPackageNames, Set<String> outInvalidPackageNames);
    public abstract ResolveInfo resolveIntent(Intent intent, String resolvedType, long flags, long privateResolveFlags, int userId, boolean resolveForStart, int filterCallingUid, int callingPid);
    public abstract ResolveInfo resolveService(Intent intent, String resolvedType, long flags, int userId, int callingUid);
    public abstract ResolveInfo resolveService(Intent intent, String resolvedType, long flags, int userId, int callingUid, int callingPid);
    public abstract ProviderInfo resolveContentProvider(String name, long flags, int userId, int callingUid);
    public abstract void addIsolatedUid(int isolatedUid, int ownerUid);
    public abstract void removeIsolatedUid(int isolatedUid);
    public abstract int getUidTargetSdkVersion(int uid);
    public abstract int getPackageTargetSdkVersion(String packageName);
    public abstract boolean canAccessInstantApps(int callingUid, int userId);
    public abstract boolean canAccessComponent(int callingUid, ComponentName component, int userId);
    public abstract boolean hasInstantApplicationMetadata(String packageName, int userId);
    public abstract void notifyPackageUse(String packageName, int reason);
    public abstract void onPackageProcessKilledForUninstall(String packageName);
    public abstract AndroidPackage getPackage(String packageName);
    public abstract AndroidPackage getAndroidPackage(String packageName);
    public abstract PackageStateInternal getPackageStateInternal(String packageName);
    public abstract ArrayMap<String, ? extends PackageStateInternal> getPackageStates();
    public abstract AndroidPackage getPackage(int uid);
    public abstract List<AndroidPackage> getPackagesForAppId(int appId);
    public PackageList getPackageList() { throw new RuntimeException("stub"); }
    public abstract PackageList getPackageList(PackageListObserver observer);
    public abstract void removePackageListObserver(PackageListObserver observer);
    public abstract PackageStateInternal getDisabledSystemPackage(String packageName);
    public abstract String getDisabledSystemPackageName(String packageName);
    public abstract boolean isResolveActivityComponent(ComponentInfo component);
    public abstract String[] getKnownPackageNames(int knownPackage, int userId);
    public abstract boolean isInstantApp(String packageName, int userId);
    public abstract String getInstantAppPackageName(int uid);
    public abstract boolean filterAppAccess(AndroidPackage pkg, int callingUid, int userId);
    public boolean filterAppAccess(String packageName, int callingUid, int userId) { throw new RuntimeException("stub"); }
    public abstract boolean filterAppAccess(String packageName, int callingUid, int userId, boolean filterUninstalled);
    public abstract boolean filterAppAccess(int uid, int callingUid);
    public abstract boolean canQueryPackage(int callingUid, String packageName);
    public abstract boolean isPlatformSigned(String pkg);
    public abstract boolean isDataRestoreSafe(byte[] restoringFromSigHash, String packageName);
    public abstract boolean isDataRestoreSafe(Signature restoringFromSig, String packageName);
    public abstract boolean hasSignatureCapability(int serverUid, int clientUid, int capability);
    public abstract String[] getSharedUserPackagesForPackage(String packageName, int userId);
    public abstract ArrayMap<String, ProcessInfo> getProcessesForUid(int uid);
    public abstract int[] getPermissionGids(String permissionName, int userId);
    public abstract void freeStorage(String volumeUuid, long bytes, int flags) throws IOException;
    public abstract void freeAllAppCacheAboveQuota(String volumeUuid) throws IOException;
    public abstract void forEachPackageSetting(Consumer<PackageSetting> actionLocked);
    public abstract void forEachPackageState(Consumer<PackageStateInternal> action);
    public abstract void forEachPackage(Consumer<AndroidPackage> action);
    public abstract void forEachInstalledPackage(Consumer<AndroidPackage> action, int userId);
    public abstract ArraySet<String> getEnabledComponents(String packageName, int userId);
    public abstract ArraySet<String> getDisabledComponents(String packageName, int userId);
    public abstract int getApplicationEnabledState(String packageName, int userId);
    public abstract int getComponentEnabledSetting(ComponentName componentName, int callingUid, int userId);
    public abstract void setEnableRollbackCode(int token, int enableRollbackCode);
    public abstract void finishPackageInstall(int token, boolean didLaunch);
    public abstract String removeLegacyDefaultBrowserPackageName(int userId);
    public abstract boolean isApexPackage(String packageName);
    public abstract List<String> getApksInApex(String apexPackageName);
    public abstract void uninstallApex(String packageName, long versionCode, int userId, IntentSender intentSender, int installFlags);
    public abstract void updateRuntimePermissionsFingerprint(int userId);
    public abstract void migrateLegacyObbData();
    public abstract void writeSettings(boolean async);
    public abstract void writePermissionSettings(int[] userIds, boolean async);
    public abstract LegacyPermissionSettings getLegacyPermissions();
    public abstract Object getLegacyPermissionsState(int userId);
    public abstract int getLegacyPermissionsVersion(int userId);
    public abstract boolean isCallerInstallerOfRecord(AndroidPackage pkg, int callingUid);
    public abstract boolean isPermissionUpgradeNeeded(int userId);
    public abstract void setVisibilityLogging(String packageName, boolean enabled);
    public abstract boolean isSystemPackage(String packageName);
    public abstract void unsuspendAdminSuspendedPackages(int userId);
    public abstract boolean isAdminSuspendingAnyPackages(int userId);
    public abstract boolean registerInstalledLoadingProgressCallback(String packageName, InstalledLoadingProgressCallback callback, int userId);
public abstract static class InstalledLoadingProgressCallback {
    private InstalledLoadingProgressCallback(android.os.Handler handler) { throw new RuntimeException("stub"); }
    public IBinder getBinder() { throw new RuntimeException("stub"); }
    public abstract void onLoadingProgressChanged(float progress);
public abstract static class LoadingProgressCallbackBinder extends android.content.pm.IPackageLoadingProgressCallback.Stub {
    private LoadingProgressCallbackBinder(InstalledLoadingProgressCallback callback) { throw new RuntimeException("stub"); }
    public void onPackageLoadingProgressChanged(float progress) { throw new RuntimeException("stub"); }
}
}
    public abstract IncrementalStatesInfo getIncrementalStatesInfo(String packageName, int filterCallingUid, int userId);
    public abstract void requestChecksums(String packageName, boolean includeSplits, int optional, int required, List trustedInstallers, IOnChecksumsReadyListener onChecksumsReadyListener, int userId, Executor executor, Handler handler);
    public abstract boolean isPackageFrozen(String packageName, int callingUid, int userId);
    public abstract long deleteOatArtifactsOfPackage(String packageName);
    public abstract void reconcileAppsData(int userId, int flags, boolean migrateAppsData);
    public abstract ArraySet<PackageStateInternal> getSharedUserPackages(int sharedUserAppId);
    public abstract SharedUserApi getSharedUserApi(int sharedUserAppId);
    public abstract boolean isUidPrivileged(int uid);
    public abstract PackageStateMutator.Result commitPackageStateMutation(PackageStateMutator.InitialState state, Consumer<PackageStateMutator> consumer);
    public abstract PackageDataSnapshot snapshot();
    public abstract void shutdown();
    public abstract DynamicCodeLogger getDynamicCodeLogger();
    public abstract int checkUidSignaturesForAllUsers(int uid1, int uid2);
    public abstract void setPackageStoppedState(String packageName, boolean stopped, int userId);
    public abstract void notifyComponentUsed(String packageName, int userId, String recentCallingPackage, String debugInfo);
    public abstract int[] getDistractingPackageRestrictionsAsUser(String[] packageNames, int userId);
    public abstract boolean isPackageStopped(String packageName, int userId) throws PackageManager.NameNotFoundException;
    public abstract void sendPackageRestartedBroadcast(String packageName, int uid, int flags);
    public abstract ParceledListSlice<PackageInstaller.SessionInfo> getHistoricalSessions(int userId);
    public abstract void sendPackageDataClearedBroadcast(String packageName, int uid, int userId, boolean isRestore, boolean isInstantApp);
    public abstract PackageArchiver getPackageArchiver();
    public abstract boolean isUpgradingFromLowerThan(int sdkVersion);
}
