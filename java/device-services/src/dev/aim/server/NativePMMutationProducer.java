package dev.aim.server;
import android.content.pm.*;
import com.android.server.pm.NativeComputer;
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
import android.content.pm.PackageManagerInternal;
import android.content.pm.PackageManagerInternal.InstalledLoadingProgressCallback;
import android.os.Binder;
import android.os.RemoteException;
import android.os.ParcelableException;

/** Concrete native operations and original helper leaves; mutation consumers stage before publication. */
public final class NativePMMutationProducer implements NativePackageManagerInternal.MutationOwner, NativePackageManagerInternal.LifecycleOwner {
    private final IPackageInternalHost nativeOwner;
    private final NativePackageStateMutation stateMutations;
    private final com.android.server.pm.NativePackageChecksums checksums;
    private final NativePackageLoadingProgress loading;
    private final com.android.server.pm.NativeInstantResolution instant;
    private final NativeSystemOverlayOwner overlays;
    public NativePMMutationProducer(IPackageInternalHost nativeOwner, NativePackageStateMutation stateMutations, com.android.server.pm.NativePackageChecksums checksums, NativePackageLoadingProgress loading, com.android.server.pm.NativeInstantResolution instant, NativeSystemOverlayOwner overlays) {
        this.nativeOwner = java.util.Objects.requireNonNull(nativeOwner);
        this.stateMutations = java.util.Objects.requireNonNull(stateMutations);
        this.checksums = java.util.Objects.requireNonNull(checksums);
        this.loading = java.util.Objects.requireNonNull(loading);
        this.instant = java.util.Objects.requireNonNull(instant);
        this.overlays = java.util.Objects.requireNonNull(overlays);
    }
    @Override public void setKeepUninstalledPackages(List<String> packageList) {
        try { nativeOwner.setKeepUninstalledPackages(packageList, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void removeAllNonSystemPackageSuspensions(int userId) {
        try { nativeOwner.removeAllNonSystemPackageSuspensions(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void removeNonSystemPackageSuspensions(String packageName, int userId) {
        try { nativeOwner.removeNonSystemPackageSuspensions(packageName, userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void removeDistractingPackageRestrictions(String packageName, int userId) {
        try { nativeOwner.removeDistractingPackageRestrictions(packageName, userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void removeAllDistractingPackageRestrictions(int userId) {
        try { nativeOwner.removeAllDistractingPackageRestrictions(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void flushPackageRestrictions(int userId) {
        try { nativeOwner.flushPackageRestrictions(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public String[] setPackagesSuspendedByAdmin(int userId, String[] packageNames, boolean suspended) {
        try { return nativeOwner.setPackagesSuspendedByAdmin(userId, packageNames, suspended, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void requestInstantAppResolutionPhaseTwo(AuxiliaryResolveInfo responseObj, Intent origIntent, String resolvedType, String callingPkg, String callingFeatureId, boolean isRequesterInstantApp, Bundle verificationBundle, int userId) {
        instant.request(responseObj, origIntent, resolvedType, callingPkg, callingFeatureId, isRequesterInstantApp, verificationBundle, userId);
    }
    @Override public void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct) {
        grantImplicitAccess(userId, intent, recipientAppId, visibleUid, direct, false);
    }
    @Override public void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct, boolean retainOnUpdate) {
        try { nativeOwner.grantImplicitAccess(userId, intent, recipientAppId, visibleUid, direct, retainOnUpdate, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void pruneInstantApps() {
        try { nativeOwner.pruneInstantApps(Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void setEnabledOverlayPackages(int userId, ArrayMap<String, OverlayPaths> pendingChanges, Set<String> outUpdatedPackageNames, Set<String> outInvalidPackageNames) {
        stateMutations.overlays(userId, pendingChanges, outUpdatedPackageNames, outInvalidPackageNames, overlays);
    }
    @Override public void addIsolatedUid(int isolatedUid, int ownerUid) {
        try { nativeOwner.addIsolatedUid(isolatedUid, ownerUid, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void removeIsolatedUid(int isolatedUid) {
        try { nativeOwner.removeIsolatedUid(isolatedUid, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void notifyPackageUse(String packageName, int reason) {
        try { nativeOwner.notifyPackageUse(packageName, reason, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void onPackageProcessKilledForUninstall(String packageName) {
        try { nativeOwner.onPackageProcessKilledForUninstall(packageName, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void freeStorage(String volumeUuid, long bytes, int flags) throws IOException {
        try { nativeOwner.freeStorage(volumeUuid, bytes, flags, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (ParcelableException failure) { failure.maybeRethrow(IOException.class); throw failure; }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void freeAllAppCacheAboveQuota(String volumeUuid) throws IOException {
        try { nativeOwner.freeAllAppCacheAboveQuota(volumeUuid, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (ParcelableException failure) { failure.maybeRethrow(IOException.class); throw failure; }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void forEachPackageSetting(Consumer<PackageSetting> actionLocked) {
        stateMutations.forEachPackageSetting(actionLocked);
    }
    @Override public void forEachInstalledPackage(Consumer<AndroidPackage> action, int userId) {
        stateMutations.forEachInstalledPackage(action, userId);
    }
    @Override public void setEnableRollbackCode(int token, int enableRollbackCode) {
        try { nativeOwner.setEnableRollbackCode(token, enableRollbackCode, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void finishPackageInstall(int token, boolean didLaunch) {
        try { nativeOwner.finishPackageInstall(token, didLaunch, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public String removeLegacyDefaultBrowserPackageName(int userId) {
        try { return nativeOwner.removeLegacyDefaultBrowserPackageName(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void uninstallApex(String packageName, long versionCode, int userId, IntentSender intentSender, int installFlags) {
        try { nativeOwner.uninstallApex(packageName, versionCode, userId, intentSender, installFlags, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void updateRuntimePermissionsFingerprint(int userId) {
        try { nativeOwner.updateRuntimePermissionsFingerprint(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void migrateLegacyObbData() {
        try { nativeOwner.migrateLegacyObbData(Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void writeSettings(boolean async) {
        try { nativeOwner.writeSettings(async, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void writePermissionSettings(int[] userIds, boolean async) {
        try { nativeOwner.writePermissionSettings(userIds, async, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void setVisibilityLogging(String packageName, boolean enabled) {
        try { nativeOwner.setVisibilityLogging(packageName, enabled, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void unsuspendAdminSuspendedPackages(int userId) {
        try { nativeOwner.unsuspendAdminSuspendedPackages(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public boolean registerInstalledLoadingProgressCallback(String packageName, InstalledLoadingProgressCallback callback, int userId) {
        return loading.register(packageName, callback, userId);
    }
    @SuppressWarnings("rawtypes")
    @Override public void requestChecksums(String packageName, boolean includeSplits, int optional, int required, List trustedInstallers, IOnChecksumsReadyListener onChecksumsReadyListener, int userId, Executor executor, Handler handler) {
        checksums.request(packageName, includeSplits, optional, required, trustedInstallers, onChecksumsReadyListener, userId, executor, handler);
    }
    @Override public long deleteOatArtifactsOfPackage(String packageName) {
        try { return nativeOwner.deleteOatArtifactsOfPackage(packageName, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void reconcileAppsData(int userId, int flags, boolean migrateAppsData) {
        try { nativeOwner.reconcileAppsData(userId, flags, migrateAppsData, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public PackageStateMutator.Result commitPackageStateMutation(PackageStateMutator.InitialState state, Consumer<PackageStateMutator> consumer) {
        return stateMutations.commit(state, consumer);
    }
    @Override public void setPackageStoppedState(String packageName, boolean stopped, int userId) {
        try { nativeOwner.setPackageStoppedState(packageName, stopped, userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void notifyComponentUsed(String packageName, int userId, String recentCallingPackage, String debugInfo) {
        try { nativeOwner.notifyComponentUsed(packageName, userId, recentCallingPackage, debugInfo, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void sendPackageRestartedBroadcast(String packageName, int uid, int flags) {
        try { nativeOwner.sendPackageRestartedBroadcast(packageName, uid, flags, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void sendPackageDataClearedBroadcast(String packageName, int uid, int userId, boolean isRestore, boolean isInstantApp) {
        try { nativeOwner.sendPackageDataClearedBroadcast(packageName, uid, userId, isRestore, isInstantApp, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void shutdown() {
        try { nativeOwner.shutdown(Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    public ParceledListSlice<PackageInstaller.SessionInfo> getHistoricalSessions(int userId) {
        PackageInstaller.SessionInfo[] entries;
        try { entries = nativeOwner.getHistoricalSessions(userId, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        if (entries == null) throw new IllegalStateException("historical native session owner unavailable");
        return new ParceledListSlice<>(java.util.Arrays.asList(entries));
    }

}
