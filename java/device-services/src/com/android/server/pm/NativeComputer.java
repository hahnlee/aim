package com.android.server.pm;
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
/** Original Computer type over a retained native capture and mandatory remaining owners. */
public final class NativeComputer implements com.android.server.pm.Computer, AutoCloseable {
    public interface Owner extends QueryOwner, ResolutionOwner, DiagnosticsOwner, dev.aim.server.NativePackageManagerInternal.PolicyReadOwner {}
    public interface QueryOwner {
        List<ResolveInfo> applyPostResolutionFilter(List<ResolveInfo> resolveInfos, String ephemeralPkgName, boolean allowDynamicSplits, int filterCallingUid, boolean resolveForStart, int userId, Intent intent);
    }
    public interface ResolutionOwner {
        CrossProfileDomainInfo getCrossProfileDomainPreferredLpr(Intent intent, String resolvedType, long flags, int sourceUserId, int parentUserId);
        ResolveInfo createForwardingResolveInfoUnchecked(WatchedIntentFilter filter, int sourceUserId, int targetUserId);
        long updateFlagsForResolve(long flags, int userId, int callingUid, boolean wantInstantApps, boolean isImplicitImageCaptureIntentAndNotSetByDpc);
        PackageManagerService.FindPreferredActivityBodyResult findPreferredActivityInternal(Intent intent, String resolvedType, long flags, List<ResolveInfo> query, boolean always, boolean removeMatches, boolean debug, int userId, boolean queryMayBeFiltered);
    }
    public interface DiagnosticsOwner {
        void dump(int type, FileDescriptor fd, PrintWriter pw, DumpState dumpState);
        void dumpPermissions(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState);
        void dumpPackages(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState, boolean checkin);
        void dumpKeySet(PrintWriter pw, String packageName, DumpState dumpState);
        void dumpSharedUsers(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState, boolean checkin);
        void dumpSharedUsersProto(ProtoOutputStream proto);
        void dumpPackagesProto(ProtoOutputStream proto);
        void dumpSharedLibrariesProto(ProtoOutputStream protoOutputStream);
    }
    public static String installerPackageName(PackageStateInternal state) {
        return state.getInstallSource().mInstallerPackageName;
    }
    private static final java.lang.ref.Cleaner CLEANER = java.lang.ref.Cleaner.create();
    private final dev.aim.server.NativePMInterfaceProducerGroups.Preferred preferred;
    private final ComponentResolverApi components;
    private final QueryOwner queries;
    private final ResolutionOwner resolution;
    private final DiagnosticsOwner diagnostics;
    private final dev.aim.server.NativePackageManagerInternal.PolicyReadOwner policy;
    private final PackageSnapshots.ComputerSnapshot snapshot;
    private final java.lang.ref.Cleaner.Cleanable lease;
    private final java.util.concurrent.atomic.AtomicInteger used = new java.util.concurrent.atomic.AtomicInteger();
    public NativeComputer(PackageSnapshots.ComputerSnapshot snapshot, Owner owner) {
        this(snapshot, owner, owner, owner, owner);
    }
    public NativeComputer(PackageSnapshots.ComputerSnapshot snapshot, QueryOwner queries,
            ResolutionOwner resolution, DiagnosticsOwner diagnostics,
            dev.aim.server.NativePackageManagerInternal.PolicyReadOwner policy) {
        this.snapshot = java.util.Objects.requireNonNull(snapshot);
        this.queries = java.util.Objects.requireNonNull(queries);
        this.resolution = java.util.Objects.requireNonNull(resolution);
        this.diagnostics = java.util.Objects.requireNonNull(diagnostics);
        this.policy = java.util.Objects.requireNonNull(policy);
        preferred = new dev.aim.server.NativePMInterfaceProducerGroups.Preferred(snapshot, this);
        components = new NativeCapturedComponentResolver(snapshot);
        lease = CLEANER.register(this, snapshot::close);
    }
    @Override public void close() { preferred.close(); lease.clean(); }
    @Override public int getVersion() { return (int) snapshot.getVersion(); }
    public long captureVersion() { return snapshot.getVersion(); }
    public long domainCaptureVersion() { return snapshot.getVersion(); }
    @Override public Computer use() { used.incrementAndGet(); return this; }
    @Override public int getUsed() { return used.get(); }
    @Override public List<ResolveInfo> queryIntentActivitiesInternal(Intent intent, String resolvedType, long flags, long privateResolveFlags, int filterCallingUid, int callingPid, int userId, boolean resolveForStart, boolean allowDynamicSplits) { return snapshot.queryIntentActivitiesInternal(intent, resolvedType, flags, privateResolveFlags, filterCallingUid, callingPid, userId, resolveForStart, allowDynamicSplits); }
    @Override public List<ResolveInfo> queryIntentActivitiesInternal(Intent intent, String resolvedType, long flags, int filterCallingUid, int userId) { return snapshot.queryIntentActivitiesInternal(intent, resolvedType, flags, 0, filterCallingUid, -1, userId, false, true); }
    @Override public List<ResolveInfo> queryIntentActivitiesInternal(Intent intent, String resolvedType, long flags, int userId) { return snapshot.queryIntentActivitiesInternal(intent, resolvedType, flags, 0, android.os.Binder.getCallingUid(), -1, userId, false, true); }
    public List<ResolveInfo> queryDomainVerificationReceivers(Intent intent, String type, long flags, int user) {
        return snapshot.queryIntentReceiversInternal(intent, type, flags,
                android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid(), user, false);
    }
    public int checkDomainVerificationPermission(String permission, String name, int user) {
        try { return snapshot.readQueries().checkPermission(permission, name, user); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public List<ResolveInfo> queryIntentServicesInternal(Intent intent, String resolvedType, long flags, int userId, int callingUid, int callingPid, boolean includeInstantApps, boolean resolveForStart) { return snapshot.queryIntentServicesInternal(intent, resolvedType, flags, userId, callingUid, callingPid, includeInstantApps, resolveForStart); }
    @Override public ActivityInfo getActivityInfo(ComponentName component, long flags, int userId) { try { return snapshot.readQueries().getActivityInfo(component, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ActivityInfo getActivityInfoInternal(ComponentName component, long flags, int filterCallingUid, int userId) { return snapshot.getActivityInfoInternal(component, flags, filterCallingUid, userId); }
    @Override public AndroidPackage getPackage(String packageName) { return snapshot.getPackage(packageName); }
    @Override public AndroidPackage getPackage(int uid) { return snapshot.getPackage(uid); }
    @Override public ApplicationInfo getApplicationInfo(String packageName, long flags, int userId) { return snapshot.getApplicationInfo(packageName, flags, android.os.Binder.getCallingUid(), userId); }
    @Override public ApplicationInfo getApplicationInfoInternal(String packageName, long flags, int filterCallingUid, int userId) { return snapshot.getApplicationInfoInternal(packageName, flags, filterCallingUid, userId); }
    @Override public ComponentName getDefaultHomeActivity(int userId) { var candidates = new java.util.ArrayList<ResolveInfo>();
        var selected = getHomeActivitiesAsUser(candidates, userId);
        if (selected != null) return selected;
        android.util.Slog.w("PackageManager", "Default package for ROLE_HOME is not set in RoleManager");
        int priority = Integer.MIN_VALUE;
        ComponentName result = null;
        for (var candidate : candidates) {
            if (candidate.priority > priority) { result = candidate.activityInfo.getComponentName(); priority = candidate.priority; }
            else if (candidate.priority == priority) result = null;
        }
        return result; }
    @Override public ComponentName getHomeActivitiesAsUser(List<ResolveInfo> allHomeCandidates, int userId) { Intent intent = getHomeIntent();
        List<ResolveInfo> candidates = queryIntentActivitiesInternal(intent,null,0, userId);
        allHomeCandidates.clear();
        if (candidates == null) return null;
        allHomeCandidates.addAll(candidates);
        String home = NativeUserManagerBridge.getDefaultHome(userId);
        if (home == null) {
            boolean filtered = android.os.UserHandle.getAppId(android.os.Binder.getCallingUid()) >= 10000;
            var selected = findPreferredActivityInternal(intent, null, 0, candidates, true, false, false, userId, filtered).mPreferredResolveInfo;
            if (selected != null && selected.activityInfo != null) home = selected.activityInfo.packageName;
        }
        if (home == null) return null;
        for (var candidate : candidates) if (candidate.activityInfo != null && home.equals(candidate.activityInfo.packageName))
            return new ComponentName(candidate.activityInfo.packageName, candidate.activityInfo.name);
        return null; }
    @Override public CrossProfileDomainInfo getCrossProfileDomainPreferredLpr(Intent intent, String resolvedType, long flags, int sourceUserId, int parentUserId) { snapshot.getVersion(); return resolution.getCrossProfileDomainPreferredLpr(intent, resolvedType, flags, sourceUserId, parentUserId); }
    @Override public Intent getHomeIntent() { snapshot.getVersion(); return new Intent("android.intent.action.MAIN").addCategory("android.intent.category.HOME").addCategory("android.intent.category.DEFAULT"); }
    @Override public List<CrossProfileIntentFilter> getMatchingCrossProfileIntentFilters(Intent intent, String resolvedType, int userId) { snapshot.getVersion(); return preferred.crossProfile(intent, resolvedType, userId); }
    @Override public List<ResolveInfo> applyPostResolutionFilter(List<ResolveInfo> resolveInfos, String ephemeralPkgName, boolean allowDynamicSplits, int filterCallingUid, boolean resolveForStart, int userId, Intent intent) { snapshot.getVersion(); return queries.applyPostResolutionFilter(resolveInfos, ephemeralPkgName, allowDynamicSplits, filterCallingUid, resolveForStart, userId, intent); }
    @Override public PackageInfo getPackageInfo(String packageName, long flags, int userId) { return snapshot.getPackageInfo(packageName, flags, android.os.Binder.getCallingUid(), userId); }
    @Override public PackageInfo getPackageInfoInternal(String packageName, long versionCode, long flags, int filterCallingUid, int userId) { return snapshot.getPackageInfoInternal(packageName, versionCode, flags, filterCallingUid, userId); }
    @Override public String[] getAllAvailablePackageNames() { return snapshot.getAllAvailablePackageNames(); }
    @Override public PackageStateInternal getPackageStateInternal(String packageName) { return snapshot.getPackageStateInternal(packageName); }
    @Override public PackageStateInternal getPackageStateInternal(String packageName, int callingUid) { return snapshot.getPackageStateInternal(packageName, callingUid); }
    @Override public PackageStateInternal getPackageStateFiltered(String packageName, int callingUid, int userId) { return snapshot.getPackageStateFiltered(packageName, callingUid, userId); }
    @Override public ParceledListSlice<PackageInfo> getInstalledPackages(long flags, int userId) { try { return snapshot.readQueries().getInstalledPackages(flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ResolveInfo createForwardingResolveInfoUnchecked(WatchedIntentFilter filter, int sourceUserId, int targetUserId) { snapshot.getVersion(); return resolution.createForwardingResolveInfoUnchecked(filter, sourceUserId, targetUserId); }
    @Override public ServiceInfo getServiceInfo(ComponentName component, long flags, int userId) { try { return snapshot.readQueries().getServiceInfo(component, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public SharedLibraryInfo getSharedLibraryInfo(String name, long version) { return snapshot.getSharedLibraryInfo(name, version); }
    @Override public String getInstantAppPackageName(int callingUid) { return snapshot.getInstantAppPackageName(callingUid); }
    @Override public String resolveInternalPackageName(String packageName, long versionCode) { return snapshot.resolveInternalPackageName(packageName, versionCode); }
    @Override public String[] getPackagesForUid(int uid) { return snapshot.getPackagesForUid(uid); }
    @Override public UserInfo getProfileParent(int userId) { snapshot.getVersion(); return NativeUserManagerBridge.getProfileParent(userId); }
    @Override public boolean canViewInstantApps(int callingUid, int userId) { return snapshot.canViewInstantApps(callingUid, userId); }
    @Override public boolean isCallerSameApp(String packageName, int uid) { return snapshot.isCallerSameApp(packageName, uid, false); }
    @Override public boolean isCallerSameApp(String packageName, int uid, boolean resolveIsolatedUid) { return snapshot.isCallerSameApp(packageName, uid, resolveIsolatedUid); }
    @Override public boolean isImplicitImageCaptureIntentAndNotSetByDpc(Intent intent, int userId, String resolvedType, long flags) { snapshot.getVersion(); return intent.isImplicitImageCaptureIntent() && !preferred.isDpmPreferred(intent, resolvedType, flags, userId); }
    @Override public boolean isInstantApp(String packageName, int userId) { return snapshot.isInstantApp(packageName, userId); }
    @Override public boolean isInstantAppInternal(String packageName, int userId, int callingUid) { return snapshot.isInstantAppInternal(packageName, userId, callingUid); }
    @Override public boolean shouldFilterApplication(PackageStateInternal ps, int callingUid, int userId) { return snapshot.shouldFilterApplication(ps, callingUid, userId, false); }
    @Override public boolean shouldFilterApplicationIncludingUninstalled(PackageStateInternal ps, int callingUid, int userId) { return snapshot.shouldFilterApplication(ps, callingUid, userId, true); }
    @Override public int checkUidPermission(String permName, int uid) { try { return snapshot.readQueries().checkUidPermission(permName, uid); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getPackageUidInternal(String packageName, long flags, int userId, int callingUid) { return snapshot.getPackageUidInternal(packageName, flags, userId, callingUid); }
    @Override public long updateFlagsForResolve(long flags, int userId, int callingUid, boolean wantInstantApps, boolean isImplicitImageCaptureIntentAndNotSetByDpc) { snapshot.getVersion(); return resolution.updateFlagsForResolve(flags, userId, callingUid, wantInstantApps, isImplicitImageCaptureIntentAndNotSetByDpc); }
    @Override public void enforceCrossUserOrProfilePermission(int callingUid, int userId, boolean requireFullPermission, boolean checkShell, String message) { snapshot.enforceCrossUserOrProfilePermission(callingUid, userId, requireFullPermission, checkShell, message); }
    @Override public void enforceCrossUserPermission(int callingUid, int userId, boolean requireFullPermission, boolean checkShell, String message) { snapshot.enforceCrossUserPermission(callingUid, userId, requireFullPermission, checkShell, message); }
    @Override public SigningDetails getSigningDetails(String packageName) { return snapshot.getSigningDetails(packageName); }
    @Override public SigningDetails getSigningDetails(int uid) { return snapshot.getSigningDetails(uid); }
    @Override public boolean filterAppAccess(AndroidPackage pkg, int callingUid, int userId) { PackageStateInternal state = snapshot.getPackageStateInternal(pkg.getPackageName()); return snapshot.shouldFilterApplication(state, callingUid, userId, true); }
    @Override public boolean filterAppAccess(String packageName, int callingUid, int userId, boolean filterUninstalled) { return snapshot.filterAppAccess(packageName, callingUid, userId, filterUninstalled); }
    @Override public boolean filterAppAccess(int uid, int callingUid) { return snapshot.filterAppAccess(uid, callingUid); }
    @Override public void dump(int type, FileDescriptor fd, PrintWriter pw, DumpState dumpState) { snapshot.getVersion(); diagnostics.dump(type, fd, pw, dumpState); }
    @Override public PackageManagerService.FindPreferredActivityBodyResult findPreferredActivityInternal(Intent intent, String resolvedType, long flags, List<ResolveInfo> query, boolean always, boolean removeMatches, boolean debug, int userId, boolean queryMayBeFiltered) { snapshot.getVersion(); return resolution.findPreferredActivityInternal(intent, resolvedType, flags, query, always, removeMatches, debug, userId, queryMayBeFiltered); }
    @Override public ResolveInfo findPersistentPreferredActivity(Intent intent, String resolvedType, long flags, List<ResolveInfo> query, boolean debug, int userId) { snapshot.getVersion(); return preferred.findPersistent(intent, resolvedType, flags, query, userId); }
    @Override public PreferredIntentResolver getPreferredActivities(int userId) { snapshot.getVersion(); return preferred.preferred(userId); }
    @Override public ArrayMap<String, ? extends PackageStateInternal> getPackageStates() { return snapshot.getPackageStates(); }
    @Override public ArrayMap<String, ? extends PackageStateInternal> getDisabledSystemPackageStates() { return snapshot.getDisabledSystemPackageStates(); }
    @Override public ArraySet<String> getNotifyPackagesForReplacedReceived(String[] packages) { return snapshot.getNotifyPackagesForReplacedReceived(packages); }
    @Override public int getPackageStartability(boolean safeMode, String packageName, int callingUid, int userId) { return snapshot.getPackageStartability(safeMode, packageName, callingUid, userId); }
    @Override public boolean isPackageAvailable(String packageName, int userId) { try { return snapshot.readQueries().isPackageAvailable(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean isApexPackage(String packageName) { return snapshot.isApexPackage(packageName); }
    @Override public String[] currentToCanonicalPackageNames(String[] names) { try { return snapshot.readQueries().currentToCanonicalPackageNames(names); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public String[] canonicalToCurrentPackageNames(String[] names) { try { return snapshot.readQueries().canonicalToCurrentPackageNames(names); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int[] getPackageGids(String packageName, long flags, int userId) { try { return snapshot.readQueries().getPackageGids(packageName, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getTargetSdkVersion(String packageName) { return snapshot.getTargetSdkVersion(packageName); }
    @Override public boolean activitySupportsIntentAsUser(ComponentName resolveComponentName, ComponentName component, Intent intent, String resolvedType, int userId) { return snapshot.activitySupportsIntentAsUser(resolveComponentName, component, intent, resolvedType, userId); }
    @Override public ActivityInfo getReceiverInfo(ComponentName component, long flags, int userId) { try { return snapshot.readQueries().getReceiverInfo(component, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ParceledListSlice<SharedLibraryInfo> getSharedLibraries(String packageName, long flags, int userId) { try { return snapshot.readQueries().getSharedLibraries(packageName, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean canRequestPackageInstalls(String packageName, int callingUid, int userId, boolean throwIfPermNotDeclared) {
        int uid = snapshot.getPackageUidInternal(packageName, 0, userId, callingUid);
        if (callingUid != uid && callingUid != 0 && callingUid != 1000)
            throw new SecurityException("Caller uid " + callingUid + " does not own package " + packageName);
        if (snapshot.isInstantAppInternal(packageName, userId, 1000)) return false;
        var pkg = snapshot.getRawPackage(packageName);
        if (pkg == null || pkg.getTargetSdkVersion() < 26) return false;
        if (!pkg.getRequestedPermissions().contains("android.permission.REQUEST_INSTALL_PACKAGES")) {
            String message = "Need to declare android.permission.REQUEST_INSTALL_PACKAGES to call this api";
            if (throwIfPermNotDeclared) throw new SecurityException(message);
            android.util.Slog.e("PackageManager", message);
            return false;
        }
        return !isInstallDisabledForPackage(packageName, uid, userId);
    }
    @Override public boolean isInstallDisabledForPackage(String packageName, int uid, int userId) { snapshot.getVersion(); return policy.isInstallDisabledForPackage(packageName, uid, userId); }
    @Override public Pair<List<VersionedPackage>, List<Boolean>> getPackagesUsingSharedLibrary(SharedLibraryInfo libInfo, long flags, int callingUid, int userId) { return snapshot.getPackagesUsingSharedLibrary(libInfo, flags, callingUid, userId); }
    @Override public ParceledListSlice<SharedLibraryInfo> getDeclaredSharedLibraries(String packageName, long flags, int userId) { try { return snapshot.readQueries().getDeclaredSharedLibraries(packageName, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ProviderInfo getProviderInfo(ComponentName component, long flags, int userId) { try { return snapshot.readQueries().getProviderInfo(component, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ArrayMap<String, String> getSystemSharedLibraryNamesAndPaths() { try { var result = new ArrayMap<String, String>(); result.putAll(snapshot.readQueries().getSystemSharedLibraryNamesAndPaths()); return result; } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public PackageStateInternal getPackageStateForInstalledAndFiltered(String packageName, int callingUid, int userId) { return snapshot.getPackageStateForInstalledAndFiltered(packageName, callingUid, userId); }
    @Override public int checkSignatures(String pkg1, String pkg2, int userId) { try { return snapshot.readQueries().checkSignatures(pkg1, pkg2, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int checkUidSignatures(int uid1, int uid2) { try { return snapshot.readQueries().checkUidSignatures(uid1, uid2); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int checkUidSignaturesForAllUsers(int uid1, int uid2) { return snapshot.checkUidSignaturesForAllUsers(uid1, uid2); }
    @Override public boolean hasSigningCertificate(String packageName, byte[] certificate, int type) { try { return snapshot.readQueries().hasSigningCertificate(packageName, certificate, type); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean hasUidSigningCertificate(int uid, byte[] certificate, int type) { try { return snapshot.readQueries().hasUidSigningCertificate(uid, certificate, type); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public List<String> getAllPackages() { try { return snapshot.readQueries().getAllPackages(); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public String getNameForUid(int uid) { return snapshot.getNameForUid(uid); }
    @Override public String[] getNamesForUids(int[] uids) { try { return snapshot.readQueries().getNamesForUids(uids); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getUidForSharedUser(String sharedUserName) { try { return snapshot.readQueries().getUidForSharedUser(sharedUserName); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getFlagsForUid(int uid) { try { return snapshot.readQueries().getFlagsForUid(uid); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getPrivateFlagsForUid(int uid) { try { return snapshot.readQueries().getPrivateFlagsForUid(uid); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean isUidPrivileged(int uid) { try { return snapshot.readQueries().isUidPrivileged(uid); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public String[] getAppOpPermissionPackages(String permissionName, int userId) { try { return snapshot.readQueries().getAppOpPermissionPackages(permissionName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ParceledListSlice<PackageInfo> getPackagesHoldingPermissions(String[] permissions, long flags, int userId) { try { return snapshot.readQueries().getPackagesHoldingPermissions(permissions, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public List<ApplicationInfo> getInstalledApplications(long flags, int userId, int callingUid, boolean forceAllowCrossUser) { return snapshot.getInstalledApplications(flags, userId, callingUid, forceAllowCrossUser); }
    @Override public ProviderInfo resolveContentProvider(String name, long flags, int userId, int callingUid) { return snapshot.resolveContentProvider(name, flags, userId, callingUid); }
    @Override public ProviderInfo resolveContentProviderForUid(String name, long flags, int userId, int filterCallingUid) { try { return snapshot.readQueries().resolveContentProviderForUid(name, flags, userId, filterCallingUid); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ProviderInfo getGrantImplicitAccessProviderInfo(int recipientUid, String visibleAuthority) { return snapshot.getGrantImplicitAccessProviderInfo(recipientUid, visibleAuthority); }
    @Override public void querySyncProviders(boolean safeMode, List<String> outNames, List<ProviderInfo> outInfo) { snapshot.querySyncProviders(safeMode, outNames, outInfo); }
    @Override public ParceledListSlice<ProviderInfo> queryContentProviders(String processName, int uid, long flags, String metaDataKey) { try { return snapshot.readQueries().queryContentProviders(processName, uid, flags, metaDataKey); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public InstrumentationInfo getInstrumentationInfoAsUser(ComponentName component, int flags, int userId) { try { return snapshot.readQueries().getInstrumentationInfoAsUser(component, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public ParceledListSlice<InstrumentationInfo> queryInstrumentationAsUser(String targetPackage, int flags, int userId) { try { return snapshot.readQueries().queryInstrumentationAsUser(targetPackage, flags, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public List<PackageStateInternal> findSharedNonSystemLibraries(PackageStateInternal pkgSetting) { return snapshot.findSharedNonSystemLibraries(pkgSetting); }
    @Override public boolean getApplicationHiddenSettingAsUser(String packageName, int userId) { try { return snapshot.readQueries().getApplicationHiddenSettingAsUser(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean isPackageSuspendedForUser(String packageName, int userId) throws PackageManager.NameNotFoundException { return snapshot.isPackageSuspendedForUser(packageName, userId); }
    @Override public boolean isPackageQuarantinedForUser(String packageName, int userId) throws PackageManager.NameNotFoundException { return snapshot.isPackageQuarantinedForUser(packageName, userId); }
    @Override public boolean isPackageStoppedForUser(String packageName, int userId) throws PackageManager.NameNotFoundException { return snapshot.isPackageStoppedForUser(packageName, userId); }
    @Override public boolean isSuspendingAnyPackages(String suspendingPackage, int suspendingUserId, int targetUserId) { return snapshot.isSuspendingAnyPackages(suspendingPackage, suspendingUserId, targetUserId); }
    @Override public ParceledListSlice<IntentFilter> getAllIntentFilters(String packageName) { try { return snapshot.readQueries().getAllIntentFilters(packageName); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean getBlockUninstallForUser(String packageName, int userId) { try { return snapshot.readQueries().getBlockUninstallForUser(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public String getInstallerPackageName(String packageName, int userId) { return snapshot.getInstallerPackageName(packageName, userId); }
    @Override public InstallSourceInfo getInstallSourceInfo(String packageName, int userId) { try { return snapshot.readQueries().getInstallSourceInfo(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getApplicationEnabledSetting(String packageName, int userId) { try { return snapshot.readQueries().getApplicationEnabledSetting(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public int getComponentEnabledSetting(ComponentName component, int callingUid, int userId) { return snapshot.getComponentEnabledSetting(component, callingUid, userId, false); }
    @Override public int getComponentEnabledSettingInternal(ComponentName component, int callingUid, int userId) { return snapshot.getComponentEnabledSetting(component, callingUid, userId, true); }
    @Override public boolean isComponentEffectivelyEnabled(ComponentInfo componentInfo, UserHandle userHandle) { return snapshot.isComponentEffectivelyEnabled(componentInfo, userHandle); }
    @Override public boolean isApplicationEffectivelyEnabled(String packageName, UserHandle userHandle) { return snapshot.isApplicationEffectivelyEnabled(packageName, userHandle); }
    @Override public KeySet getKeySetByAlias(String packageName, String alias) { try { return snapshot.readQueries().getKeySetByAlias(packageName, alias); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public KeySet getSigningKeySet(String packageName) { try { return snapshot.readQueries().getSigningKeySet(packageName); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean isPackageSignedByKeySet(String packageName, KeySet ks) { try { return snapshot.readQueries().isPackageSignedByKeySet(packageName, ks); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean isPackageSignedByKeySetExactly(String packageName, KeySet ks) { try { return snapshot.readQueries().isPackageSignedByKeySetExactly(packageName, ks); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public SparseArray<int[]> getVisibilityAllowLists(String packageName, int[] userIds) { return snapshot.getVisibilityAllowLists(packageName, userIds); }
    @Override public int[] getVisibilityAllowList(String packageName, int userId) { return snapshot.getVisibilityAllowList(packageName, userId); }
    @Override public boolean canQueryPackage(int callingUid, String targetPackageName) { return snapshot.canQueryPackage(callingUid, targetPackageName); }
    @Override public int getPackageUid(String packageName, long flags, int userId) { return snapshot.getPackageUid(packageName, flags, userId); }
    @Override public boolean canAccessComponent(int callingUid, ComponentName component, int userId) { return snapshot.canAccessComponent(callingUid, component, userId); }
    @Override public boolean isCallerInstallerOfRecord(AndroidPackage pkg, int callingUid) { return snapshot.isCallerInstallerOfRecord(pkg, callingUid); }
    @Override public int getInstallReason(String packageName, int userId) { try { return snapshot.readQueries().getInstallReason(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean[] canPackageQuery(String sourcePackageName, String[] targetPackageNames, int userId) { try { return snapshot.readQueries().canPackageQuery(sourcePackageName, targetPackageNames, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public boolean canForwardTo(Intent intent, String resolvedType, int sourceUserId, int targetUserId) { try { return snapshot.readQueries().canForwardTo(intent, resolvedType, sourceUserId, targetUserId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public List<ApplicationInfo> getPersistentApplications(boolean safeMode, int flags) { return snapshot.getPersistentApplications(safeMode, flags); }
    @Override public String[] getSharedUserPackagesForPackage(String packageName, int userId) { return snapshot.getSharedUserPackagesForPackage(packageName, userId); }
    @Override public CharSequence getHarmfulAppWarning(String packageName, int userId) { try { return snapshot.readQueries().getHarmfulAppWarning(packageName, userId); } catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); } }
    @Override public String[] filterOnlySystemPackages(String[] pkgNames) { return snapshot.filterOnlySystemPackages(pkgNames); }
    @Override public List<AndroidPackage> getPackagesForAppId(int appId) { return snapshot.getPackagesForAppId(appId); }
    @Override public int getUidTargetSdkVersion(int uid) { return snapshot.getUidTargetSdkVersion(uid); }
    @Override public ArrayMap<String, ProcessInfo> getProcessesForUid(int uid) { return snapshot.getProcessesForUid(uid); }
    @Override public boolean getBlockUninstall(int userId, String packageName) { return snapshot.getBlockUninstall(userId, packageName); }
    @Override public WatchedArrayMap<String, WatchedLongSparseArray<SharedLibraryInfo>> getSharedLibraries() { return snapshot.getSharedLibraries(); }
    @Override public Pair<PackageStateInternal, SharedUserApi> getPackageOrSharedUser(int appId) { return snapshot.getPackageOrSharedUser(appId); }
    @Override public SharedUserApi getSharedUser(int sharedUserAppIde) { return snapshot.getSharedUser(sharedUserAppIde); }
    @Override public ArraySet<PackageStateInternal> getSharedUserPackages(int sharedUserAppId) { return snapshot.getSharedUserPackages(sharedUserAppId); }
    @Override public ComponentResolverApi getComponentResolver() { snapshot.getVersion(); return components; }
    @Override public PackageStateInternal getDisabledSystemPackage(String packageName) { return snapshot.getDisabledSystemPackage(packageName); }
    @Override public ResolveInfo getInstantAppInstallerInfo() { return snapshot.getInstantAppInstallerInfo(); }
    @Override public WatchedArrayMap<String, Integer> getFrozenPackages() { return snapshot.getFrozenPackages(); }
    @Override public void checkPackageFrozen(String packageName) { snapshot.checkPackageFrozen(packageName); }
    @Override public ComponentName getInstantAppInstallerComponent() { return snapshot.getInstantAppInstallerComponent(); }
    @Override public void dumpPermissions(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState) { snapshot.getVersion(); diagnostics.dumpPermissions(pw, packageName, permissionNames, dumpState); }
    @Override public void dumpPackages(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState, boolean checkin) { snapshot.getVersion(); diagnostics.dumpPackages(pw, packageName, permissionNames, dumpState, checkin); }
    @Override public void dumpKeySet(PrintWriter pw, String packageName, DumpState dumpState) { snapshot.getVersion(); diagnostics.dumpKeySet(pw, packageName, dumpState); }
    @Override public void dumpSharedUsers(PrintWriter pw, String packageName, ArraySet<String> permissionNames, DumpState dumpState, boolean checkin) { snapshot.getVersion(); diagnostics.dumpSharedUsers(pw, packageName, permissionNames, dumpState, checkin); }
    @Override public void dumpSharedUsersProto(ProtoOutputStream proto) { snapshot.getVersion(); diagnostics.dumpSharedUsersProto(proto); }
    @Override public void dumpPackagesProto(ProtoOutputStream proto) { snapshot.getVersion(); diagnostics.dumpPackagesProto(proto); }
    @Override public void dumpSharedLibrariesProto(ProtoOutputStream protoOutputStream) { snapshot.getVersion(); diagnostics.dumpSharedLibrariesProto(protoOutputStream); }
    @Override public List<? extends PackageStateInternal> getVolumePackages(String volumeUuid) { return snapshot.getVolumePackages(volumeUuid); }
    @Override public UserInfo[] getUserInfos() { snapshot.getVersion(); return NativeUserManagerBridge.getUserInfos(); }
    @Override public ArrayMap<String, ? extends SharedUserApi> getSharedUsers() { return snapshot.getSharedUsers(); }
}
