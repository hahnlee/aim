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
/** Original Internal type; mandatory owners supply operations outside the retained read slice. */
public final class NativePackageManagerInternal extends android.content.pm.PackageManagerInternal {
    public interface Owner extends ReadOwner, MutationOwner, LifecycleOwner, PolicyOwner, PolicyReadOwner, ObserverOwner {}
    public interface ObserverOwner {
        void addPackageListObserver(PackageListObserver observer);
        void removePackageListObserver(PackageListObserver observer);
    }
    public interface PolicyReadOwner {
        boolean isInstallDisabledForPackage(String packageName, int uid, int userId);
        boolean isPackageDataProtected(int userId, String packageName);
        boolean isPackageStateProtected(String packageName, int userId);
    }
    public interface PolicyOwner {
        void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwner, SparseArray<String> profileOwners);
        void setOwnerProtectedPackages(int userId, List<String> packageNames);
        void setExternalSourcesPolicy(ExternalSourcesPolicy policy);
    }
    public interface LegacyRuntimeOwner {
        Object getLegacyPermissionsState(int userId);
        int getLegacyPermissionsVersion(int userId);
    }
    public interface ReadOwner extends LegacyRuntimeOwner {
        LegacyPermissionSettings getLegacyPermissions();
        DynamicCodeLogger getDynamicCodeLogger();
        ParceledListSlice<PackageInstaller.SessionInfo> getHistoricalSessions(int userId);
        PackageArchiver getPackageArchiver();
    }
    public interface MutationOwner {
        void setKeepUninstalledPackages(List<String> packageList);
        void removeAllNonSystemPackageSuspensions(int userId);
        void removeNonSystemPackageSuspensions(String packageName, int userId);
        void removeDistractingPackageRestrictions(String packageName, int userId);
        void removeAllDistractingPackageRestrictions(int userId);
        void flushPackageRestrictions(int userId);
        String[] setPackagesSuspendedByAdmin(int userId, String[] packageNames, boolean suspended);
        void requestInstantAppResolutionPhaseTwo(AuxiliaryResolveInfo responseObj, Intent origIntent, String resolvedType, String callingPkg, String callingFeatureId, boolean isRequesterInstantApp, Bundle verificationBundle, int userId);
        void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct);
        void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct, boolean retainOnUpdate);
        void pruneInstantApps();
        void setEnabledOverlayPackages(int userId, ArrayMap<String, OverlayPaths> pendingChanges, Set<String> outUpdatedPackageNames, Set<String> outInvalidPackageNames);
        void addIsolatedUid(int isolatedUid, int ownerUid);
        void removeIsolatedUid(int isolatedUid);
        void notifyPackageUse(String packageName, int reason);
        void onPackageProcessKilledForUninstall(String packageName);
        void freeStorage(String volumeUuid, long bytes, int flags) throws IOException;
        void freeAllAppCacheAboveQuota(String volumeUuid) throws IOException;
        void forEachPackageSetting(Consumer<PackageSetting> actionLocked);
        void forEachInstalledPackage(Consumer<AndroidPackage> action, int userId);
        void setEnableRollbackCode(int token, int enableRollbackCode);
        void finishPackageInstall(int token, boolean didLaunch);
        String removeLegacyDefaultBrowserPackageName(int userId);
        void uninstallApex(String packageName, long versionCode, int userId, IntentSender intentSender, int installFlags);
        void updateRuntimePermissionsFingerprint(int userId);
        void migrateLegacyObbData();
        void writeSettings(boolean async);
        void writePermissionSettings(int[] userIds, boolean async);
        void setVisibilityLogging(String packageName, boolean enabled);
        void unsuspendAdminSuspendedPackages(int userId);
        boolean registerInstalledLoadingProgressCallback(String packageName, InstalledLoadingProgressCallback callback, int userId);
        @SuppressWarnings("rawtypes")
        void requestChecksums(String packageName, boolean includeSplits, int optional, int required, List trustedInstallers, IOnChecksumsReadyListener onChecksumsReadyListener, int userId, Executor executor, Handler handler);
        long deleteOatArtifactsOfPackage(String packageName);
        void reconcileAppsData(int userId, int flags, boolean migrateAppsData);
        PackageStateMutator.Result commitPackageStateMutation(PackageStateMutator.InitialState state, Consumer<PackageStateMutator> consumer);
        void setPackageStoppedState(String packageName, boolean stopped, int userId);
        void notifyComponentUsed(String packageName, int userId, String recentCallingPackage, String debugInfo);
        void sendPackageRestartedBroadcast(String packageName, int uid, int flags);
        void sendPackageDataClearedBroadcast(String packageName, int uid, int userId, boolean isRestore, boolean isInstantApp);
    }
    public interface LifecycleOwner {
        void shutdown();
    }

    // PackageManagerService.ensureSystemPackageName, pinned AOSP (Apache 2.0).
    private static String ensureSystemPackageName(PackageSnapshots.ComputerSnapshot scope, String name) {
        if (name == null) return null;
        long token = android.os.Binder.clearCallingIdentity();
        try {
            if (scope.getPackageInfo(name, 0x200000L, 1000, 0) == null) {
                var info = scope.getPackageInfo(name, 0, 1000, 0);
                if (info != null) android.util.EventLog.writeEvent(0x534e4554, "145981139", info.applicationInfo.uid, "");
                android.util.Log.w("PackageManager", "Missing required system package: " + name
                        + (info != null ? ", but found with extended search." : "."));
                return null;
            }
        } finally { android.os.Binder.restoreCallingIdentity(token); }
        return name;
    }
    private record Bound(PackageSnapshots.Store snapshots, ReadOwner reads,
            LegacyRuntimeOwner legacyRuntime, MutationOwner mutations, LifecycleOwner lifecycle,
            PolicyOwner policy, PolicyReadOwner policyReads, ObserverOwner observers,
            java.util.function.Function<PackageSnapshots.ComputerSnapshot, NativeComputer> computers) {}
    private volatile Bound bound;
    private volatile IPackageBootSession initialQueries;
    private volatile PackageLocal initialLocal;
    private volatile LegacyPermissionSettings initialPermissions;
    private volatile LegacyRuntimeOwner initialLegacyRuntime;
    private volatile boolean closed;
    /** Stable identity retained by original permission constructors before scan capture. */
    public NativePackageManagerInternal() {}
    private Bound state() {
        Bound value = bound;
        if (closed || value == null) throw new IllegalStateException("native package internal epoch is not ready");
        return value;
    }
    public synchronized void bindInitialQueries(IPackageBootSession queries) {
        if (closed || bound != null || initialQueries != null) throw new IllegalStateException("initial package queries already bound or closed");
        initialQueries = java.util.Objects.requireNonNull(queries);
    }
    public synchronized void bindInitialQueries(IPackageBootSession queries,PackageLocal local,LegacyPermissionSettings permissions) {
        bindInitialQueries(queries);initialLocal=java.util.Objects.requireNonNull(local);initialPermissions=java.util.Objects.requireNonNull(permissions);
    }
    public synchronized void bindInitialLegacyRuntime(LegacyRuntimeOwner owner){
        if(closed||bound!=null||initialQueries==null||initialLegacyRuntime!=null)throw new IllegalStateException("Initial legacy runtime owner already bound or unavailable");
        initialLegacyRuntime=java.util.Objects.requireNonNull(owner);
    }
    private LegacyRuntimeOwner initialRuntime(){initial();return java.util.Objects.requireNonNull(initialLegacyRuntime,"Initial legacy runtime owner unavailable");}
    private PackageStateInternal initialPackage(String name){
        initial();var local=java.util.Objects.requireNonNull(initialLocal,"Initial package Local owner unavailable");
        try(var scope=local.withUnfilteredSnapshot()){return (PackageStateInternal)scope.getPackageStates().get(name);}
    }
    private IPackageBootSession initial() {
        IPackageBootSession value = initialQueries;
        if (closed || value == null) throw new IllegalStateException("native raw package queries not ready");
        return value;
    }
    public synchronized void closeEpoch() { closed = true; bound = null; initialQueries = null; initialLocal=null; initialPermissions=null;initialLegacyRuntime=null; }
    public NativePackageManagerInternal(PackageSnapshots.Store snapshots, Owner owner,
            java.util.function.Function<PackageSnapshots.ComputerSnapshot, NativeComputer.Owner> computerOwners) {
        this(snapshots, owner, owner, owner, owner, owner, owner, computerFactory(computerOwners), owner);
    }
    private static java.util.function.Function<PackageSnapshots.ComputerSnapshot, NativeComputer> computerFactory(
            java.util.function.Function<PackageSnapshots.ComputerSnapshot, NativeComputer.Owner> owners) {
        java.util.Objects.requireNonNull(owners);
        return scope -> new NativeComputer(scope, owners.apply(scope));
    }
    public NativePackageManagerInternal(PackageSnapshots.Store snapshots, ReadOwner reads,
            MutationOwner mutations, LifecycleOwner lifecycle, PolicyOwner policy,
            PolicyReadOwner policyReads, ObserverOwner observers,
            java.util.function.Function<PackageSnapshots.ComputerSnapshot, NativeComputer> computers,
            LegacyRuntimeOwner legacyRuntime) {
        bind(snapshots, reads, mutations, lifecycle, policy, policyReads, observers, computers, legacyRuntime);
    }
    /** Publish all genuine owners at once; no method can see a partial binding. */
    public synchronized void bind(PackageSnapshots.Store snapshots, ReadOwner reads,
            MutationOwner mutations, LifecycleOwner lifecycle, PolicyOwner policy,
            PolicyReadOwner policyReads, ObserverOwner observers,
            java.util.function.Function<PackageSnapshots.ComputerSnapshot, NativeComputer> computers,
            LegacyRuntimeOwner legacyRuntime) {
        if (closed || bound != null) throw new IllegalStateException("native package internal already bound or closed");
        bound = new Bound(java.util.Objects.requireNonNull(snapshots), java.util.Objects.requireNonNull(reads),
                java.util.Objects.requireNonNull(legacyRuntime), java.util.Objects.requireNonNull(mutations),
                java.util.Objects.requireNonNull(lifecycle), java.util.Objects.requireNonNull(policy),
                java.util.Objects.requireNonNull(policyReads), java.util.Objects.requireNonNull(observers),
                java.util.Objects.requireNonNull(computers));
    }

    @Override public void setKeepUninstalledPackages(List<String> packageList) { state(); state().mutations().setKeepUninstalledPackages(packageList); }
    @Override public boolean isPermissionsReviewRequired(String packageName, int userId) { state(); return com.android.server.pm.NativeUserManagerBridge.isPermissionsReviewRequired(packageName, userId); }
    @Override public boolean isSameApp(String packageName, int callingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.isSameApp(packageName, callingUid, userId); } }
    @Override public boolean isSameApp(String packageName, long flags, int callingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.isSameApp(packageName, flags, callingUid, userId); } }
    @Override public PackageInfo getPackageInfo(String packageName, long flags, int filterCallingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getPackageInfo(packageName, flags, filterCallingUid, userId); } }
    @Override public List<ApplicationInfo> getInstalledApplications(long flags, int userId, int callingUid) { state(); try (var scope = state().snapshots().computer()) { return scope.getInstalledApplications(flags, userId, callingUid, false); } }
    @Override public List<ApplicationInfo> getInstalledApplicationsCrossUser(long flags, int userId, int callingUid) { state(); try (var scope = state().snapshots().computer()) { return scope.getInstalledApplications(flags, userId, callingUid, true); } }
    @Override public Bundle getSuspendedPackageLauncherExtras(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getSuspendedPackageLauncherExtras(packageName, userId); } }
    @Override public boolean isPackageSuspended(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.isPackageSuspended(packageName, userId); } }
    @Override public void removeAllNonSystemPackageSuspensions(int userId) { state(); state().mutations().removeAllNonSystemPackageSuspensions(userId); }
    @Override public void removeNonSystemPackageSuspensions(String packageName, int userId) { state(); state().mutations().removeNonSystemPackageSuspensions(packageName, userId); }
    @Override public void removeDistractingPackageRestrictions(String packageName, int userId) { state(); state().mutations().removeDistractingPackageRestrictions(packageName, userId); }
    @Override public void removeAllDistractingPackageRestrictions(int userId) { state(); state().mutations().removeAllDistractingPackageRestrictions(userId); }
    @Override public void flushPackageRestrictions(int userId) { state(); state().mutations().flushPackageRestrictions(userId); }
    @Override public UserPackage getSuspendingPackage(String suspendedPackage, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getSuspendingPackage(suspendedPackage, userId); } }
    @Override public String[] setPackagesSuspendedByAdmin(int userId, String[] packageNames, boolean suspended) { state(); return state().mutations().setPackagesSuspendedByAdmin(userId, packageNames, suspended); }
    @Override public SuspendDialogInfo getSuspendedDialogInfo(String suspendedPackage, UserPackage suspendingPackage, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getSuspendedDialogInfo(suspendedPackage, suspendingPackage, userId); } }
    @Override public int getDistractingPackageRestrictions(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { var state = scope.getPackageStateInternal(packageName); return state == null ? 0 : state.getUserStateOrDefault(userId).getDistractionFlags(); } }
    @Override public int getPackageUid(String packageName, long flags, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getPackageUidInternal(packageName, flags, userId); } }
    @Override public ApplicationInfo getApplicationInfo(String packageName, long flags, int filterCallingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getApplicationInfo(packageName, flags, filterCallingUid, userId); } }
    @Override public ActivityInfo getActivityInfo(ComponentName component, long flags, int filterCallingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getActivityInfoInternal(component, flags, filterCallingUid, userId); } }
    @Override public List<ResolveInfo> queryIntentActivities(Intent intent, String resolvedType, long flags, int filterCallingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.queryIntentActivitiesInternal(intent, resolvedType, flags, 0, filterCallingUid, -1, userId, false, true); } }
    @Override public List<ResolveInfo> queryIntentReceivers(Intent intent, String resolvedType, long flags, int filterCallingUid, int callingPid, int userId, boolean forSend) { state(); try (var scope = state().snapshots().computer()) { return scope.queryIntentReceiversInternal(intent, resolvedType, flags, filterCallingUid, callingPid, userId, forSend); } }
    @Override public List<ResolveInfo> queryIntentServices(Intent intent, long flags, int callingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.queryIntentServicesInternal(intent, com.android.server.pm.NativeUserManagerBridge.resolveTypeIfNeeded(intent), flags, userId, callingUid, -1, false, false); } }
    @Override public ComponentName getHomeActivitiesAsUser(List<ResolveInfo> allHomeCandidates, int userId) { state(); try (var computer = (NativeComputer) snapshot()) { return computer.getHomeActivitiesAsUser(allHomeCandidates, userId); } }
    @Override public ComponentName getDefaultHomeActivity(int userId) { state(); try (var computer = (NativeComputer) snapshot()) { return computer.getDefaultHomeActivity(userId); } }
    @Override public ComponentName getSystemUiServiceComponent() { state(); return com.android.server.pm.NativeUserManagerBridge.getSystemUiServiceComponent(); }
    @Override public void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwner, SparseArray<String> profileOwners) { state(); state().policy().setDeviceAndProfileOwnerPackages(deviceOwnerUserId, deviceOwner, profileOwners); }
    @Override public void setOwnerProtectedPackages(int userId, List<String> packageNames) { state(); state().policy().setOwnerProtectedPackages(userId, packageNames); }
    @Override public boolean isPackageDataProtected(int userId, String packageName) { state(); return state().policyReads().isPackageDataProtected(userId, packageName); }
    @Override public boolean isPackageStateProtected(String packageName, int userId) { state(); return state().policyReads().isPackageStateProtected(packageName, userId); }
    @Override public boolean isPackageEphemeral(int userId, String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.isPackageEphemeral(userId, packageName); } }
    @Override public boolean wasPackageEverLaunched(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.wasPackageEverLaunched(packageName, userId); } }
    @Override public String getNameForUid(int uid) { state(); try (var scope = state().snapshots().computer()) { return scope.getNameForUid(uid); } }
    @Override public void requestInstantAppResolutionPhaseTwo(AuxiliaryResolveInfo responseObj, Intent origIntent, String resolvedType, String callingPkg, String callingFeatureId, boolean isRequesterInstantApp, Bundle verificationBundle, int userId) { state(); state().mutations().requestInstantAppResolutionPhaseTwo(responseObj, origIntent, resolvedType, callingPkg, callingFeatureId, isRequesterInstantApp, verificationBundle, userId); }
    @Override public void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct) { state(); state().mutations().grantImplicitAccess(userId, intent, recipientAppId, visibleUid, direct); }
    @Override public void grantImplicitAccess(int userId, Intent intent, int recipientAppId, int visibleUid, boolean direct, boolean retainOnUpdate) { state(); state().mutations().grantImplicitAccess(userId, intent, recipientAppId, visibleUid, direct, retainOnUpdate); }
    @Override public boolean isInstantAppInstallerComponent(ComponentName component) { state(); try (var scope = state().snapshots().computer()) { var installer = scope.getInstantAppInstallerComponent(); return installer != null && installer.equals(component); } }
    @Override public void pruneInstantApps() { state(); state().mutations().pruneInstantApps(); }
    @Override public String getSetupWizardPackageName() { state(); try (var scope = state().snapshots().computer()) { return scope.getSetupWizardPackageName(); } }
    @Override public void setExternalSourcesPolicy(ExternalSourcesPolicy policy) { state(); state().policy().setExternalSourcesPolicy(policy); }
    @Override public boolean isPackagePersistent(String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.isPackagePersistent(packageName); } }
    @Override public List<String> getTargetPackageNames(int userId) { state(); try (var scope = state().snapshots().computer()) { var names = new java.util.ArrayList<String>(); scope.forEachPackage(pkg -> { if (!pkg.isResourceOverlay()) names.add(pkg.getPackageName()); }); return names; } }
    @Override public void setEnabledOverlayPackages(int userId, ArrayMap<String, OverlayPaths> pendingChanges, Set<String> outUpdatedPackageNames, Set<String> outInvalidPackageNames) { state(); state().mutations().setEnabledOverlayPackages(userId, pendingChanges, outUpdatedPackageNames, outInvalidPackageNames); }
    @Override public ResolveInfo resolveIntent(Intent intent, String resolvedType, long flags, long privateResolveFlags, int userId, boolean resolveForStart, int filterCallingUid, int callingPid) { state(); try (var scope = state().snapshots().computer()) { return scope.resolveIntentInternal(intent, resolvedType, flags, privateResolveFlags, userId, resolveForStart, filterCallingUid, callingPid); } }
    @Override public ResolveInfo resolveService(Intent intent, String resolvedType, long flags, int userId, int callingUid) { state(); try (var scope = state().snapshots().computer()) { return scope.resolveServiceInternal(intent, resolvedType, flags, userId, callingUid, -1, false); } }
    @Override public ResolveInfo resolveService(Intent intent, String resolvedType, long flags, int userId, int callingUid, int callingPid) { state(); try (var scope = state().snapshots().computer()) { return scope.resolveServiceInternal(intent, resolvedType, flags, userId, callingUid, callingPid, true); } }
    @Override public ProviderInfo resolveContentProvider(String name, long flags, int userId, int callingUid) { state(); try (var scope = state().snapshots().computer()) { return scope.resolveContentProvider(name, flags, userId, callingUid); } }
    @Override public void addIsolatedUid(int isolatedUid, int ownerUid) { state(); state().mutations().addIsolatedUid(isolatedUid, ownerUid); }
    @Override public void removeIsolatedUid(int isolatedUid) { state(); state().mutations().removeIsolatedUid(isolatedUid); }
    @Override public int getUidTargetSdkVersion(int uid) { state(); try (var scope = state().snapshots().computer()) { return scope.getUidTargetSdkVersion(uid); } }
    @Override public int getPackageTargetSdkVersion(String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.getPackageTargetSdkVersion(packageName); } }
    @Override public boolean canAccessInstantApps(int callingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.canViewInstantApps(callingUid, userId); } }
    @Override public boolean canAccessComponent(int callingUid, ComponentName component, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.canAccessComponent(callingUid, component, userId); } }
    @Override public boolean hasInstantApplicationMetadata(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.hasInstantApplicationMetadata(packageName, userId); } }
    @Override public void notifyPackageUse(String packageName, int reason) { state(); state().mutations().notifyPackageUse(packageName, reason); }
    @Override public void onPackageProcessKilledForUninstall(String packageName) { state(); state().mutations().onPackageProcessKilledForUninstall(packageName); }
    @Override public AndroidPackage getPackage(String packageName) { if(bound==null){var packageState=initialPackage(packageName);return packageState==null?null:packageState.getPkg();}state(); try (var scope = state().snapshots().computer()) { return scope.getPackage(packageName); } }
    @Override public AndroidPackage getAndroidPackage(String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.getAndroidPackage(packageName); } }
    @Override public PackageStateInternal getPackageStateInternal(String packageName) { if(bound==null)return initialPackage(packageName);state(); try (var scope = state().snapshots().computer()) { return scope.getPackageStateInternal(packageName); } }
    @Override public ArrayMap<String, ? extends PackageStateInternal> getPackageStates() { if(bound==null){initial();var result=new ArrayMap<String,PackageStateInternal>();try(var scope=java.util.Objects.requireNonNull(initialLocal).withUnfilteredSnapshot()){scope.getPackageStates().forEach((name,value)->result.put(name,(PackageStateInternal)value));}return result;}state(); try (var scope = state().snapshots().computer()) { return scope.getPackageStates(); } }
    @Override public AndroidPackage getPackage(int uid) { state(); try (var scope = state().snapshots().computer()) { return scope.getPackage(uid); } }
    @Override public List<AndroidPackage> getPackagesForAppId(int appId) { state(); try (var scope = state().snapshots().computer()) { return scope.getPackagesForAppId(appId); } }
    @Override public PackageList getPackageList(PackageListObserver observer) { state(); try (var scope = state().snapshots().computer()) {
            var names = new java.util.ArrayList<String>();
            scope.forEachPackage(pkg -> names.add(pkg.getPackageName()));
            var result = new PackageList(names, observer);
            if (observer != null) state().observers().addPackageListObserver(result);
            return result;
        } }
    @Override public void removePackageListObserver(PackageListObserver observer) { state(); state().observers().removePackageListObserver(observer); }
    @Override public PackageStateInternal getDisabledSystemPackage(String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.getDisabledSystemPackage(packageName); } }
    @Override public String getDisabledSystemPackageName(String packageName) { state(); try (var scope = state().snapshots().computer()) { var state = scope.getDisabledSystemPackage(packageName); var pkg = state == null ? null : state.getPkg(); return pkg == null ? null : pkg.getPackageName(); } }
    @Override public boolean isResolveActivityComponent(ComponentInfo component) { state(); try (var scope = state().snapshots().computer()) { return scope.isResolveActivityComponent(component); } }
    @Override public String[] getKnownPackageNames(int knownPackage, int userId) { if(bound==null){try{return initial().getInitialKnownPackageNames(knownPackage,userId);}catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}} state(); try (var scope = state().snapshots().computer()) { return scope.getKnownPackageNames(knownPackage, userId); } }
    @Override public boolean isInstantApp(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.isInstantApp(packageName, userId); } }
    @Override public String getInstantAppPackageName(int uid) {
        if (bound == null) try { return initial().getInitialInstantAppPackageName(uid); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        state(); try (var scope = state().snapshots().computer()) { return scope.getInstantAppPackageName(uid); } }
    @Override public boolean filterAppAccess(AndroidPackage pkg, int callingUid, int userId) {
        if (bound == null) try { return initial().filterInitialPackageAccess(java.util.Objects.requireNonNull(pkg).getPackageName(), callingUid, userId, true); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        state(); try (var scope = state().snapshots().computer()) { var state = scope.getPackageStateInternal(pkg.getPackageName()); return scope.shouldFilterApplication(state, callingUid, userId, true); } }
    @Override public boolean filterAppAccess(String packageName, int callingUid, int userId, boolean filterUninstalled) {
        if (bound == null) try { return initial().filterInitialPackageAccess(packageName, callingUid, userId, filterUninstalled); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        state(); try (var scope = state().snapshots().computer()) { return scope.filterAppAccess(packageName, callingUid, userId, filterUninstalled); } }
    @Override public boolean filterAppAccess(int uid, int callingUid) {
        if (bound == null) try { return initial().filterInitialUidAccess(uid, callingUid); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        state(); try (var scope = state().snapshots().computer()) { return scope.filterAppAccess(uid, callingUid); } }
    @Override public boolean canQueryPackage(int callingUid, String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.canQueryPackage(callingUid, packageName); } }
    @Override public boolean isPlatformSigned(String pkg) { state(); try (var scope = state().snapshots().computer()) { var state = scope.getPackageStateInternal(pkg); if (state == null) return false; var signing = state.getSigningDetails(); var platform = scope.getPlatformSigningDetails(); return signing.hasAncestorOrSelf(platform) || platform.checkCapability(signing, 4); } }
    @Override public boolean isDataRestoreSafe(byte[] restoringFromSigHash, String packageName) { state(); try (var scope = state().snapshots().computer()) { var signing = scope.getSigningDetails(packageName); return signing != null && signing.hasSha256Certificate(restoringFromSigHash, 1); } }
    @Override public boolean isDataRestoreSafe(Signature restoringFromSig, String packageName) { state(); try (var scope = state().snapshots().computer()) { var signing = scope.getSigningDetails(packageName); return signing != null && signing.hasCertificate(restoringFromSig, 1); } }
    @Override public boolean hasSignatureCapability(int serverUid, int clientUid, int capability) { state(); try (var scope = state().snapshots().computer()) { var server = scope.getSigningDetails(serverUid); var client = scope.getSigningDetails(clientUid); return server.checkCapability(client, capability) || client.hasAncestorOrSelf(server); } }
    @Override public String[] getSharedUserPackagesForPackage(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getSharedUserPackagesForPackage(packageName, userId); } }
    @Override public ArrayMap<String, ProcessInfo> getProcessesForUid(int uid) { state(); try (var scope = state().snapshots().computer()) { return scope.getProcessesForUid(uid); } }
    @Override public int[] getPermissionGids(String permissionName, int userId) { state(); return com.android.server.pm.NativeUserManagerBridge.getPermissionGids(permissionName, userId); }
    @Override public void freeStorage(String volumeUuid, long bytes, int flags) throws IOException { state(); state().mutations().freeStorage(volumeUuid, bytes, flags); }
    @Override public void freeAllAppCacheAboveQuota(String volumeUuid) throws IOException { state(); state().mutations().freeAllAppCacheAboveQuota(volumeUuid); }
    @Override public void forEachPackageSetting(Consumer<PackageSetting> actionLocked) { state(); state().mutations().forEachPackageSetting(actionLocked); }
    @Override public void forEachPackageState(Consumer<PackageStateInternal> action) { state(); try (var scope = state().snapshots().computer()) { scope.forEachPackageState(action); } }
    @Override public void forEachPackage(Consumer<AndroidPackage> action) { state(); try (var scope = state().snapshots().computer()) { scope.forEachPackage(action); } }
    @Override public void forEachInstalledPackage(Consumer<AndroidPackage> action, int userId) { state(); state().mutations().forEachInstalledPackage(action, userId); }
    @Override public ArraySet<String> getEnabledComponents(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getEnabledComponents(packageName, userId); } }
    @Override public ArraySet<String> getDisabledComponents(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getDisabledComponents(packageName, userId); } }
    @Override public int getApplicationEnabledState(String packageName, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getApplicationEnabledState(packageName, userId); } }
    @Override public int getComponentEnabledSetting(ComponentName componentName, int callingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.getComponentEnabledSetting(componentName, callingUid, userId, true); } }
    @Override public void setEnableRollbackCode(int token, int enableRollbackCode) { state(); state().mutations().setEnableRollbackCode(token, enableRollbackCode); }
    @Override public void finishPackageInstall(int token, boolean didLaunch) { state(); state().mutations().finishPackageInstall(token, didLaunch); }
    @Override public String removeLegacyDefaultBrowserPackageName(int userId) { state(); return state().mutations().removeLegacyDefaultBrowserPackageName(userId); }
    @Override public boolean isApexPackage(String packageName) { state(); try (var scope = state().snapshots().computer()) { return scope.isApexPackage(packageName); } }
    @Override public List<String> getApksInApex(String apexPackageName) { state(); try (var scope = state().snapshots().computer()) { return scope.getApksInApex(apexPackageName); } }
    @Override public void uninstallApex(String packageName, long versionCode, int userId, IntentSender intentSender, int installFlags) { state(); state().mutations().uninstallApex(packageName, versionCode, userId, intentSender, installFlags); }
    @Override public void updateRuntimePermissionsFingerprint(int userId) { state(); state().mutations().updateRuntimePermissionsFingerprint(userId); }
    @Override public void migrateLegacyObbData() { state(); state().mutations().migrateLegacyObbData(); }
    @Override public void writeSettings(boolean async) { state(); state().mutations().writeSettings(async); }
    @Override public void writePermissionSettings(int[] userIds, boolean async) { state(); state().mutations().writePermissionSettings(userIds, async); }
    @Override public LegacyPermissionSettings getLegacyPermissions() { if(bound==null){initial();return java.util.Objects.requireNonNull(initialPermissions,"Initial legacy definitions owner unavailable");} state(); return state().reads().getLegacyPermissions(); }
    @Override public Object getLegacyPermissionsState(int userId) { if(bound==null)return initialRuntime().getLegacyPermissionsState(userId);state(); return state().legacyRuntime().getLegacyPermissionsState(userId); }
    @Override public int getLegacyPermissionsVersion(int userId) { if(bound==null)return initialRuntime().getLegacyPermissionsVersion(userId);state(); return state().legacyRuntime().getLegacyPermissionsVersion(userId); }
    @Override public boolean isCallerInstallerOfRecord(AndroidPackage pkg, int callingUid) { state(); try (var scope = state().snapshots().computer()) { return scope.isCallerInstallerOfRecord(pkg, callingUid); } }
    @Override public boolean isPermissionUpgradeNeeded(int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.isPermissionUpgradeNeeded(userId); } }
    @Override public void setVisibilityLogging(String packageName, boolean enabled) { state(); state().mutations().setVisibilityLogging(packageName, enabled); }
    @Override public boolean isSystemPackage(String packageName) { state(); try (var scope = state().snapshots().computer()) { return packageName.equals(ensureSystemPackageName(scope, packageName)); } }
    @Override public void unsuspendAdminSuspendedPackages(int userId) { state(); state().mutations().unsuspendAdminSuspendedPackages(userId); }
    @Override public boolean isAdminSuspendingAnyPackages(int userId) { state(); try (var scope = state().snapshots().computer()) { return scope.isSuspendingAnyPackages("android", state().snapshots().crossUserSuspensions() ? 0 : userId, userId); } }
    @Override public boolean registerInstalledLoadingProgressCallback(String packageName, InstalledLoadingProgressCallback callback, int userId) { state(); return state().mutations().registerInstalledLoadingProgressCallback(packageName, callback, userId); }
    @Override public IncrementalStatesInfo getIncrementalStatesInfo(String packageName, int filterCallingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { var state = scope.getPackageStateForInstalledAndFiltered(packageName, filterCallingUid, userId); return state == null ? null : new IncrementalStatesInfo(state.isLoading(), state.getLoadingProgress(), state.getLoadingCompletedTime()); } }
    @SuppressWarnings("rawtypes")
    @Override public void requestChecksums(String packageName, boolean includeSplits, int optional, int required, List trustedInstallers, IOnChecksumsReadyListener onChecksumsReadyListener, int userId, Executor executor, Handler handler) { state(); state().mutations().requestChecksums(packageName, includeSplits, optional, required, trustedInstallers, onChecksumsReadyListener, userId, executor, handler); }
    @Override public boolean isPackageFrozen(String packageName, int callingUid, int userId) { state(); try (var scope = state().snapshots().computer()) { try { return scope.getPackageStartability(scope.readQueries().isSafeMode(), packageName, callingUid, userId) == 3; } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } } }
    @Override public long deleteOatArtifactsOfPackage(String packageName) { state(); return state().mutations().deleteOatArtifactsOfPackage(packageName); }
    @Override public void reconcileAppsData(int userId, int flags, boolean migrateAppsData) { state(); state().mutations().reconcileAppsData(userId, flags, migrateAppsData); }
    @Override public ArraySet<PackageStateInternal> getSharedUserPackages(int sharedUserAppId) { state(); try (var scope = state().snapshots().computer()) { return scope.getSharedUserPackages(sharedUserAppId); } }
    @Override public SharedUserApi getSharedUserApi(int sharedUserAppId) { state(); try (var scope = state().snapshots().computer()) { return scope.getSharedUserApi(sharedUserAppId); } }
    @Override public boolean isUidPrivileged(int uid) { state(); try (var scope = state().snapshots().computer()) { try { return scope.readQueries().isUidPrivileged(uid); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } } }
    @Override public PackageStateMutator.Result commitPackageStateMutation(PackageStateMutator.InitialState state, Consumer<PackageStateMutator> consumer) { state(); return state().mutations().commitPackageStateMutation(state, consumer); }
    @Override public PackageDataSnapshot snapshot() { state(); var scope = state().snapshots().computer(); try { return java.util.Objects.requireNonNull(state().computers().apply(scope)); } catch (RuntimeException failure) { scope.close(); throw failure; } }
    @Override public void shutdown() { state(); state().lifecycle().shutdown(); }
    @Override public DynamicCodeLogger getDynamicCodeLogger() { state(); return state().reads().getDynamicCodeLogger(); }
    @Override public int checkUidSignaturesForAllUsers(int uid1, int uid2) { state(); try (var scope = state().snapshots().computer()) { return scope.checkUidSignaturesForAllUsers(uid1, uid2); } }
    @Override public void setPackageStoppedState(String packageName, boolean stopped, int userId) { state(); state().mutations().setPackageStoppedState(packageName, stopped, userId); }
    @Override public void notifyComponentUsed(String packageName, int userId, String recentCallingPackage, String debugInfo) { state(); state().mutations().notifyComponentUsed(packageName, userId, recentCallingPackage, debugInfo); }
    @Override public int[] getDistractingPackageRestrictionsAsUser(String[] packageNames, int userId) { state(); try (var scope = state().snapshots().computer()) {
            java.util.Objects.requireNonNull(packageNames, "packageNames cannot be null");
            int[] result = new int[packageNames.length]; java.util.Arrays.fill(result, -1);
            int uid = android.os.Binder.getCallingUid();
            for (int i = 0; i < packageNames.length; i++) {
                var state = scope.getPackageStateForInstalledAndFiltered(packageNames[i], uid, userId);
                if (state != null) result[i] = state.getUserStateOrDefault(userId).getDistractionFlags();
            }
            return result;
        } }
    @Override public boolean isPackageStopped(String packageName, int userId) throws PackageManager.NameNotFoundException { state(); try (var scope = state().snapshots().computer()) { return scope.isPackageStoppedForUser(packageName, userId); } }
    @Override public void sendPackageRestartedBroadcast(String packageName, int uid, int flags) { state(); state().mutations().sendPackageRestartedBroadcast(packageName, uid, flags); }
    @Override public ParceledListSlice<PackageInstaller.SessionInfo> getHistoricalSessions(int userId) { state(); return state().reads().getHistoricalSessions(userId); }
    @Override public void sendPackageDataClearedBroadcast(String packageName, int uid, int userId, boolean isRestore, boolean isInstantApp) { state(); state().mutations().sendPackageDataClearedBroadcast(packageName, uid, userId, isRestore, isInstantApp); }
    @Override public PackageArchiver getPackageArchiver() { state(); return state().reads().getPackageArchiver(); }
    @Override public boolean isUpgradingFromLowerThan(int sdkVersion) { state(); try (var scope = state().snapshots().computer()) { return scope.isUpgradingFromLowerThan(sdkVersion); } }
}
