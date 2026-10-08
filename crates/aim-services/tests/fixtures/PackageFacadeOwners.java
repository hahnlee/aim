package dev.aim.server;
// Explicit test-only owners; no production completion or fallback semantics.
import android.content.pm.PackageManagerInternal.*;
import android.content.pm.*;
import com.android.server.pm.*;
import dev.aim.server.PackageSnapshots;
import android.content.ComponentName;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ActivityInfo;
import android.content.pm.ApplicationInfo;
import android.content.pm.ComponentInfo;
import android.content.pm.InstallSourceInfo;
import android.content.pm.InstrumentationInfo;
import android.content.pm.KeySet;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.ParceledListSlice;
import android.content.pm.ProcessInfo;
import android.content.pm.ProviderInfo;
import android.content.pm.ResolveInfo;
import android.content.pm.ServiceInfo;
import android.content.pm.SharedLibraryInfo;
import android.content.pm.SigningDetails;
import android.content.pm.UserInfo;
import android.content.pm.VersionedPackage;
import android.os.UserHandle;
import android.util.ArrayMap;
import android.util.ArraySet;
import android.util.Pair;
import android.util.SparseArray;
import android.util.proto.ProtoOutputStream;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.SharedUserApi;
import com.android.server.pm.resolution.ComponentResolverApi;
import com.android.server.pm.snapshot.PackageDataSnapshot;
import com.android.server.utils.WatchedArrayMap;
import com.android.server.utils.WatchedLongSparseArray;
import java.io.FileDescriptor;
import java.io.PrintWriter;
import java.util.List;
import java.util.Set;
import com.android.server.pm.Computer;
import com.android.server.pm.NativeComputer;
import android.content.IntentSender;
import android.content.pm.overlay.OverlayPaths;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import com.android.internal.pm.pkg.component.ParsedMainComponent;
import com.android.server.pm.PackageArchiver;
import com.android.server.pm.PackageList;
import com.android.server.pm.PackageSetting;
import com.android.server.pm.dex.DynamicCodeLogger;
import com.android.server.pm.permission.LegacyPermissionSettings;
import com.android.server.pm.pkg.mutate.PackageStateMutator;
import java.io.IOException;
import java.util.concurrent.Executor;
import java.util.function.Consumer;
import android.content.pm.PackageManagerInternal;
public final class PackageFacadeOwners {
 private PackageFacadeOwners() {}
 public static final class ComputerOwner implements NativeComputer.Owner {
  @Override public boolean isInstallDisabledForPackage(String name, int uid, int user) { throw new AssertionError("unexpected mandatory policy read: install"); }
  @Override public boolean isPackageDataProtected(int user, String name) { throw new AssertionError("unexpected mandatory policy read: data"); }
  @Override public boolean isPackageStateProtected(String name, int user) { throw new AssertionError("unexpected mandatory policy read: state"); }
  @Override public List<ResolveInfo> applyPostResolutionFilter(List<ResolveInfo> resolveInfos, String ephemeralPkgName, boolean allowDynamicSplits, int filterCallingUid, boolean resolveForStart, int userId, Intent intent) { throw new AssertionError("unexpected mandatory owner call: applyPostResolutionFilter"); }
  @Override public CrossProfileDomainInfo getCrossProfileDomainPreferredLpr(Intent intent, String resolvedType, long flags, int sourceUserId, int parentUserId) { throw new AssertionError("unexpected mandatory owner call: getCrossProfileDomainPreferredLpr"); }
  @Override public ResolveInfo createForwardingResolveInfoUnchecked(WatchedIntentFilter filter, int sourceUserId, int targetUserId) { throw new AssertionError("unexpected mandatory owner call: createForwardingResolveInfoUnchecked"); }
  @Override public long updateFlagsForResolve(long flags, int userId, int callingUid, boolean wantInstantApps, boolean isImplicitImageCaptureIntentAndNotSetByDpc) { throw new AssertionError("unexpected mandatory owner call: updateFlagsForResolve"); }
  @Override public PackageManagerService.FindPreferredActivityBodyResult findPreferredActivityInternal(Intent intent, String resolvedType, long flags, List<ResolveInfo> query, boolean always, boolean removeMatches, boolean debug, int userId, boolean queryMayBeFiltered) { throw new AssertionError("unexpected mandatory owner call: findPreferredActivityInternal"); }
  @Override public void dump(int type, FileDescriptor fd, PrintWriter pw, DumpState dumpState) { throw new AssertionError("unexpected mandatory owner call: dump"); }
  @Override public void dumpPermissions(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState) { throw new AssertionError("unexpected mandatory owner call: dumpPermissions"); }
  @Override public void dumpPackages(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState, boolean checkin) { throw new AssertionError("unexpected mandatory owner call: dumpPackages"); }
  @Override public void dumpKeySet(PrintWriter pw, String packageName, DumpState dumpState) { throw new AssertionError("unexpected mandatory owner call: dumpKeySet"); }
  @Override public void dumpSharedUsers(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState, boolean checkin) { throw new AssertionError("unexpected mandatory owner call: dumpSharedUsers"); }
  @Override public void dumpSharedUsersProto(ProtoOutputStream proto) { throw new AssertionError("unexpected mandatory owner call: dumpSharedUsersProto"); }
  @Override public void dumpPackagesProto(ProtoOutputStream proto) { throw new AssertionError("unexpected mandatory owner call: dumpPackagesProto"); }
  @Override public void dumpSharedLibrariesProto(ProtoOutputStream protoOutputStream) { throw new AssertionError("unexpected mandatory owner call: dumpSharedLibrariesProto"); }
 }
 public static final class InternalOwner implements NativePackageManagerInternal.Owner {
  @Override public void addPackageListObserver(PackageListObserver observer) { throw new AssertionError("unexpected mandatory owner call: addPackageListObserver"); }
  @Override public void removePackageListObserver(PackageListObserver observer) { throw new AssertionError("unexpected mandatory owner call: removePackageListObserver"); }
  @Override public boolean isInstallDisabledForPackage(String packageName, int uid, int userId) { throw new AssertionError("unexpected mandatory owner call: isInstallDisabledForPackage"); }
  @Override public boolean isPackageDataProtected(int userId, String packageName) { throw new AssertionError("unexpected mandatory owner call: isPackageDataProtected"); }
  @Override public boolean isPackageStateProtected(String packageName, int userId) { throw new AssertionError("unexpected mandatory owner call: isPackageStateProtected"); }
  @Override public void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwner, SparseArray<String> profileOwners) { throw new AssertionError("unexpected mandatory owner call: setDeviceAndProfileOwnerPackages"); }
  @Override public void setOwnerProtectedPackages(int userId, List<String> packageNames) { throw new AssertionError("unexpected mandatory owner call: setOwnerProtectedPackages"); }
  @Override public void setExternalSourcesPolicy(ExternalSourcesPolicy policy) { throw new AssertionError("unexpected mandatory owner call: setExternalSourcesPolicy"); }
  @Override public LegacyPermissionSettings getLegacyPermissions() { throw new AssertionError("unexpected mandatory owner call: getLegacyPermissions"); }
  @Override public Object getLegacyPermissionsState(int userId) { throw new AssertionError("unexpected mandatory owner call: getLegacyPermissionsState"); }
  @Override public int getLegacyPermissionsVersion(int userId) { throw new AssertionError("unexpected mandatory owner call: getLegacyPermissionsVersion"); }
  @Override public DynamicCodeLogger getDynamicCodeLogger() { throw new AssertionError("unexpected mandatory owner call: getDynamicCodeLogger"); }
  @Override public ParceledListSlice<PackageInstaller.SessionInfo> getHistoricalSessions(int userId) { throw new AssertionError("unexpected mandatory owner call: getHistoricalSessions"); }
  @Override public PackageArchiver getPackageArchiver() { throw new AssertionError("unexpected mandatory owner call: getPackageArchiver"); }
  @Override public void setKeepUninstalledPackages(List<String> packageList) { throw new AssertionError("unexpected mandatory owner call: setKeepUninstalledPackages"); }
  @Override public void removeAllNonSystemPackageSuspensions(int userId) { throw new AssertionError("unexpected mandatory owner call: removeAllNonSystemPackageSuspensions"); }
  @Override public void removeNonSystemPackageSuspensions(String packageName, int userId) { throw new AssertionError("unexpected mandatory owner call: removeNonSystemPackageSuspensions"); }
  @Override public void removeDistractingPackageRestrictions(String packageName, int userId) { throw new AssertionError("unexpected mandatory owner call: removeDistractingPackageRestrictions"); }
  @Override public void removeAllDistractingPackageRestrictions(int userId) { throw new AssertionError("unexpected mandatory owner call: removeAllDistractingPackageRestrictions"); }
  @Override public void flushPackageRestrictions(int userId) { throw new AssertionError("unexpected mandatory owner call: flushPackageRestrictions"); }
  @Override public String[] setPackagesSuspendedByAdmin(int userId, String[] packageNames, boolean suspended) { throw new AssertionError("unexpected mandatory owner call: setPackagesSuspendedByAdmin"); }
  @Override public void requestInstantAppResolutionPhaseTwo(AuxiliaryResolveInfo responseObj, Intent origIntent, String resolvedType, String callingPkg, String callingFeatureId, boolean isRequesterInstantApp, Bundle verificationBundle, int userId) { throw new AssertionError("unexpected mandatory owner call: requestInstantAppResolutionPhaseTwo"); }
  @Override public void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct) { throw new AssertionError("unexpected mandatory owner call: grantImplicitAccess"); }
  @Override public void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct, boolean retainOnUpdate) { throw new AssertionError("unexpected mandatory owner call: grantImplicitAccess"); }
  @Override public void pruneInstantApps() { throw new AssertionError("unexpected mandatory owner call: pruneInstantApps"); }
  @Override public void setEnabledOverlayPackages(int userId, ArrayMap<String, OverlayPaths> pendingChanges, Set<String> outUpdatedPackageNames, Set<String> outInvalidPackageNames) { throw new AssertionError("unexpected mandatory owner call: setEnabledOverlayPackages"); }
  @Override public void addIsolatedUid(int isolatedUid, int ownerUid) { throw new AssertionError("unexpected mandatory owner call: addIsolatedUid"); }
  @Override public void removeIsolatedUid(int isolatedUid) { throw new AssertionError("unexpected mandatory owner call: removeIsolatedUid"); }
  @Override public void notifyPackageUse(String packageName, int reason) { throw new AssertionError("unexpected mandatory owner call: notifyPackageUse"); }
  @Override public void onPackageProcessKilledForUninstall(String packageName) { throw new AssertionError("unexpected mandatory owner call: onPackageProcessKilledForUninstall"); }
  @Override public void freeStorage(String volumeUuid, long bytes, int flags) throws IOException { throw new AssertionError("unexpected mandatory owner call: freeStorage"); }
  @Override public void freeAllAppCacheAboveQuota(String volumeUuid) throws IOException { throw new AssertionError("unexpected mandatory owner call: freeAllAppCacheAboveQuota"); }
  @Override public void forEachPackageSetting(Consumer<PackageSetting> actionLocked) { throw new AssertionError("unexpected mandatory owner call: forEachPackageSetting"); }
  @Override public void forEachInstalledPackage(Consumer<AndroidPackage> action, int userId) { throw new AssertionError("unexpected mandatory owner call: forEachInstalledPackage"); }
  @Override public void setEnableRollbackCode(int token, int enableRollbackCode) { throw new AssertionError("unexpected mandatory owner call: setEnableRollbackCode"); }
  @Override public void finishPackageInstall(int token, boolean didLaunch) { throw new AssertionError("unexpected mandatory owner call: finishPackageInstall"); }
  @Override public String removeLegacyDefaultBrowserPackageName(int userId) { throw new AssertionError("unexpected mandatory owner call: removeLegacyDefaultBrowserPackageName"); }
  @Override public void uninstallApex(String packageName, long versionCode, int userId, IntentSender intentSender, int installFlags) { throw new AssertionError("unexpected mandatory owner call: uninstallApex"); }
  @Override public void updateRuntimePermissionsFingerprint(int userId) { throw new AssertionError("unexpected mandatory owner call: updateRuntimePermissionsFingerprint"); }
  @Override public void migrateLegacyObbData() { throw new AssertionError("unexpected mandatory owner call: migrateLegacyObbData"); }
  @Override public void writeSettings(boolean async) { throw new AssertionError("unexpected mandatory owner call: writeSettings"); }
  @Override public void writePermissionSettings(int[] userIds, boolean async) { throw new AssertionError("unexpected mandatory owner call: writePermissionSettings"); }
  @Override public void setVisibilityLogging(String packageName, boolean enabled) { throw new AssertionError("unexpected mandatory owner call: setVisibilityLogging"); }
  @Override public void unsuspendAdminSuspendedPackages(int userId) { throw new AssertionError("unexpected mandatory owner call: unsuspendAdminSuspendedPackages"); }
  @Override public boolean registerInstalledLoadingProgressCallback(String packageName, InstalledLoadingProgressCallback callback, int userId) { throw new AssertionError("unexpected mandatory owner call: registerInstalledLoadingProgressCallback"); }
  @Override public void requestChecksums(String packageName, boolean includeSplits, int optional, int required, List trustedInstallers, IOnChecksumsReadyListener onChecksumsReadyListener, int userId, Executor executor, Handler handler) { throw new AssertionError("unexpected mandatory owner call: requestChecksums"); }
  @Override public long deleteOatArtifactsOfPackage(String packageName) { throw new AssertionError("unexpected mandatory owner call: deleteOatArtifactsOfPackage"); }
  @Override public void reconcileAppsData(int userId, int flags, boolean migrateAppsData) { throw new AssertionError("unexpected mandatory owner call: reconcileAppsData"); }
  @Override public PackageStateMutator.Result commitPackageStateMutation(PackageStateMutator.InitialState state, Consumer<PackageStateMutator> consumer) { throw new AssertionError("unexpected mandatory owner call: commitPackageStateMutation"); }
  @Override public void setPackageStoppedState(String packageName, boolean stopped, int userId) { throw new AssertionError("unexpected mandatory owner call: setPackageStoppedState"); }
  @Override public void notifyComponentUsed(String packageName, int userId, String recentCallingPackage, String debugInfo) { throw new AssertionError("unexpected mandatory owner call: notifyComponentUsed"); }
  @Override public void sendPackageRestartedBroadcast(String packageName, int uid, int flags) { throw new AssertionError("unexpected mandatory owner call: sendPackageRestartedBroadcast"); }
  @Override public void sendPackageDataClearedBroadcast(String packageName, int uid, int userId, boolean isRestore, boolean isInstantApp) { throw new AssertionError("unexpected mandatory owner call: sendPackageDataClearedBroadcast"); }
  @Override public void shutdown() { throw new AssertionError("unexpected mandatory owner call: shutdown"); }
 }
}
