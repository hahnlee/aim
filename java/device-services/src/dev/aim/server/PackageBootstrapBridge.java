package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.IBinder;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import com.android.server.LocalServices;
import com.android.server.compat.PlatformCompat;
import com.android.server.pm.parsing.library.PackageBackwardCompatibility;
import com.android.server.pm.parsing.PackageCacher;
import com.android.server.pm.parsing.pkg.AndroidPackageUtils;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.pm.verify.domain.DomainVerificationManagerInternal;

/** Original package-policy owners, available independently of late services. */
public final class PackageBootstrapBridge extends IPackageBootstrapBridge.Stub {
    private final DomainVerificationManagerInternal domains;

    public PackageBootstrapBridge() { this(null); }
    public PackageBootstrapBridge(DomainVerificationManagerInternal domains) {
        this.domains = domains;
    }

    private static final long ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES = 142191088L;
    private static final long SELINUX_LATEST_CHANGES = 143539591L;
    private static final long SELINUX_R_CHANGES = 168782947L;

    /** Early M4 policy handoff; original PMS remains until the C facade passes its gates (#798). */
    public static com.android.server.pm.PackageManagerService startPackageManager(Context context,
            com.android.server.pm.Installer installer,
            com.android.server.pm.verify.domain.DomainVerificationService domains,
            boolean factoryTest) {
        try {
            attach(domains);
        } catch (RemoteException failure) {
            throw new IllegalStateException("package bootstrap attach failed", failure);
        }
        return com.android.server.pm.PackageManagerService.main(context, installer, domains, factoryTest);
    }

    /** Called by the C facade before native scanning, after PlatformCompat starts. */
    public static void attach(DomainVerificationManagerInternal domains) throws RemoteException {
        if (domains == null) throw new IllegalArgumentException("missing domain owner");
        IBinder host = ServiceManager.checkService("aim.service_host");
        if (host == null) throw new IllegalStateException("native service host is unavailable");
        IServiceHost.Stub.asInterface(host).attachPackageBootstrapBridge(new PackageBootstrapBridge(domains));
    }

    /** Capture a complete native replica before exposing the facade's scopes. */
    public static PackageSnapshots.Store captureSnapshots(PackageSnapshots.Owner owner,
            boolean crossUserSuspensions) throws RemoteException, java.io.IOException {
        IBinder binder = ServiceManager.checkService("aim.service_host");
        if (binder == null) throw new IllegalStateException("native service host is unavailable");
        var host = IServiceHost.Stub.asInterface(binder);
        var snapshots = new PackageSnapshots.Store(host::capturePackageScan, owner, crossUserSuspensions);
        snapshots.refresh();
        return snapshots;
    }

    /** SDK data goes to the native install owner, never to original PMS. */
    public static PackageLocal.SdkDataOwner sdkDataOwner() {
        IBinder binder = ServiceManager.checkService("aim.service_host");
        if (binder == null) throw new IllegalStateException("native service host is unavailable");
        return sdkDataOwner(IServiceHost.Stub.asInterface(binder));
    }

    private static PackageLocal.SdkDataOwner sdkDataOwner(IServiceHost host) {
        return (volume, name, dirs, user, app, previous, seinfo, flags) -> {
            try {
                host.reconcilePackageSdkData(volume, name, dirs, user, app, previous, seinfo, flags);
            } catch (Exception failure) {
                // InstallerException.from followed by PMS's IOException uses this exact message.
                throw new java.io.IOException(failure.toString());
            }
        };
    }

    public static PackageLocal.SigningOwner signingOwner() {
        IBinder binder = ServiceManager.checkService("aim.service_host");
        if (binder == null) throw new IllegalStateException("native service host is unavailable");
        return signingOwner(IServiceHost.Stub.asInterface(binder));
    }

    private static PackageLocal.SigningOwner signingOwner(IServiceHost host) {
        return new PackageLocal.SigningOwner() {
            public void add(android.content.pm.SigningDetails oldDetails, android.content.pm.SigningDetails newDetails) {
                try { host.addPackageSigningOverride(PackageSigningDetails.encode(oldDetails), PackageSigningDetails.encode(newDetails)); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
            public void remove(android.content.pm.SigningDetails oldDetails) {
                try { host.removePackageSigningOverride(PackageSigningDetails.encode(oldDetails)); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
            public void clear() {
                try { host.clearPackageSigningOverrides(); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
        };
    }

    public static PackageLocal captureLocal(PackageSnapshots.Owner owner,
            boolean crossUserSuspensions) throws RemoteException, java.io.IOException {
        IBinder binder = ServiceManager.checkService("aim.service_host");
        if (binder == null) throw new IllegalStateException("native service host is unavailable");
        var host = IServiceHost.Stub.asInterface(binder);
        var snapshots = new PackageSnapshots.Store(host::capturePackageScan, owner, crossUserSuspensions);
        snapshots.refresh();
        return new PackageLocal(snapshots, sdkDataOwner(host), signingOwner(host));
    }

    @Override
    public boolean isSigningDebuggable() {
        enforceSystemUid();
        return android.os.Build.isDebuggable();
    }

    @Override
    public boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk) {
        enforceSystemUid();
        PlatformCompat compat = (PlatformCompat) ServiceManager.getService(
                Context.PLATFORM_COMPAT_SERVICE);
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        return compat.isChangeEnabledInternal(
                ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES, packageName, targetSdk);
    }

    @Override
    public boolean isApplicationQueryFilteringEnabled(String packageName, int targetSdk) throws RemoteException {
        enforceSystemUid();
        if (packageName == null || packageName.isEmpty() || targetSdk < 0)
            throw new IllegalArgumentException("invalid query compatibility identity");
        var info = new android.content.pm.ApplicationInfo();
        info.packageName = packageName;
        info.targetSdkVersion = targetSdk;
        var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                ServiceManager.getService(Context.PLATFORM_COMPAT_SERVICE));
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        return compat.getAppConfig(info).isChangeEnabled(135549675L);
    }

    @Override
    public boolean isDomainVerifierUid(int uid) {
        enforceSystemUid();
        if (uid < 0) throw new IllegalArgumentException("invalid domain verifier UID");
        if (domains == null) throw new IllegalStateException("domain owner is unavailable");
        var proxy = domains.getProxy();
        if (proxy == null) throw new IllegalStateException("domain proxy is unavailable");
        return proxy.isCallerVerifier(uid);
    }

    @Override
    public void invalidatePackageInfoCache() {
        enforceSystemUid();
        android.content.pm.PackageManager.invalidatePackageInfoCache();
    }

    @Override
    public boolean isDomainVerificationRestricted(String packageName, int targetSdk) throws RemoteException {
        return domainCompatibility(175408749L, packageName, targetSdk);
    }

    @Override
    public boolean isDomainVerificationSettingsV2Enabled(String packageName, int targetSdk) throws RemoteException {
        return domainCompatibility(178111421L, packageName, targetSdk);
    }

    private static boolean domainCompatibility(long changeId, String packageName, int targetSdk) throws RemoteException {
        enforceSystemUid();
        if (packageName == null || packageName.isEmpty() || targetSdk < 0)
            throw new IllegalArgumentException("invalid domain compatibility identity");
        // DomainVerificationUtils.buildMockAppInfo supplies precisely these fields.
        var info = new android.content.pm.ApplicationInfo();
        info.packageName = packageName;
        info.targetSdkVersion = targetSdk;
        var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                ServiceManager.getService(Context.PLATFORM_COMPAT_SERVICE));
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        return compat.getAppConfig(info).isChangeEnabled(changeId);
    }

    @Override
    public boolean isTestBaseLibraryChangeEnabled(byte[] packageCache) throws RemoteException {
        enforceSystemUid();
        if (packageCache == null) throw new IllegalArgumentException("missing parsed package");
        AndroidPackage pkg = (AndroidPackage) PackageCacher.fromCacheEntryStatic(packageCache);
        var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                ServiceManager.getService(Context.PLATFORM_COMPAT_SERVICE));
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        return compat.isChangeEnabled(133396946L, AndroidPackageUtils.generateAppInfoWithoutState(pkg));
    }

    @Override
    public boolean isSharedUidMigrationBestEffort() {
        enforceSystemUid();
        // android-16.0.0_r1 SharedUidMigration.BEST_EFFORT.
        return com.android.server.pm.SharedUidMigration.applyStrategy(2);
    }

    @Override
    public boolean isTestBaseOnBootclasspath() {
        enforceSystemUid();
        return PackageBackwardCompatibility.bootClassPathContainsATB();
    }

    @Override
    public int getSeInfoTargetSdkVersion(byte[] packageCache) {
        enforceSystemUid();
        if (packageCache == null) throw new IllegalArgumentException("missing parsed package");
        AndroidPackage pkg = (AndroidPackage) PackageCacher.fromCacheEntryStatic(packageCache);
        android.content.pm.ApplicationInfo appInfo = AndroidPackageUtils.generateAppInfoWithoutState(pkg);
        PlatformCompat compat = (PlatformCompat) ServiceManager.getService(
                Context.PLATFORM_COMPAT_SERVICE);
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        if (compat.isChangeEnabledInternal(SELINUX_LATEST_CHANGES, appInfo)) {
            return Math.max(android.os.Build.VERSION_CODES.CUR_DEVELOPMENT, pkg.getTargetSdkVersion());
        }
        if (compat.isChangeEnabledInternal(SELINUX_R_CHANGES, appInfo)) {
            return Math.max(android.os.Build.VERSION_CODES.R, pkg.getTargetSdkVersion());
        }
        return pkg.getTargetSdkVersion();
    }

    @Override
    public int[] getPermissionGidsForUid(int uid) {
        enforceSystemUid();
        if (uid < 0) throw new IllegalArgumentException("negative permission UID");
        PermissionManagerServiceInternal permissions = LocalServices.getService(
                PermissionManagerServiceInternal.class);
        if (permissions == null) throw new IllegalStateException("permission owner is unavailable");
        return permissions.getGidsForUid(uid);
    }

    @Override
    public String[] getPackageInstalledPermissions(String packageName) {
        enforceSystemUid();
        if (packageName == null || packageName.isEmpty()) throw new IllegalArgumentException("missing permission package");
        return permissionNames(permissionOwner().getInstalledPermissions(packageName));
    }

    @Override
    public String[] getPackageGrantedPermissions(String packageName, int appId, int userId) {
        enforceSystemUid();
        if (packageName == null || packageName.isEmpty() || appId < 0 || appId >= 100000 || userId < 0)
            throw new IllegalArgumentException("invalid permission identity");
        var permissions = permissionOwner();
        var local = com.android.server.LocalManagerRegistry.getManager(com.android.server.pm.PackageManagerLocal.class);
        if (local == null) throw new IllegalStateException("package local owner is unavailable");
        try (var snapshot = local.withUnfilteredSnapshot()) {
            var state = snapshot.getPackageStates().get(packageName);
            if (state == null || state.getAppId() != appId)
                throw new IllegalStateException("permission UID owner differs");
            String[] granted = permissionNames(permissions.getGrantedPermissions(packageName, userId));
            try (var current = local.withUnfilteredSnapshot()) {
                if (current.getPackageStates().get(packageName) != state)
                    throw new IllegalStateException("permission package owner changed");
            }
            return granted;
        }
    }

    private static PermissionManagerServiceInternal permissionOwner() {
        var owner = LocalServices.getService(PermissionManagerServiceInternal.class);
        if (owner == null) throw new IllegalStateException("permission owner is unavailable");
        return owner;
    }

    private static String[] permissionNames(java.util.Set<String> names) {
        if (names == null) throw new IllegalStateException("missing permission names");
        for (String name : names) if (name == null) throw new IllegalStateException("null permission name");
        return new java.util.TreeSet<>(names).toArray(new String[0]);
    }

    @Override
    public byte[] getLegacyPermissionState(int appId, int[] userIds) {
        enforceSystemUid();
        PackageLegacyPermissions.validate(appId, userIds);
        PermissionManagerServiceInternal permissions = LocalServices.getService(
                PermissionManagerServiceInternal.class);
        if (permissions == null) throw new IllegalStateException("permission owner is unavailable");
        return PackageLegacyPermissions.capture(appId, userIds, permissions.getLegacyPermissionState(appId));
    }

    @Override
    public byte[] generateNewDomainId() {
        enforceSystemUid();
        return PackageDomainIds.generate(domains);
    }

    @Override
    public byte[] getPackageScanUsers() {
        enforceSystemUid();
        return PackageScanUsers.capture();
    }

    @Override
    public byte[] getApexBootInventory() {
        enforceSystemUid();
        return com.android.server.pm.ApexBootFeed.capture();
    }

    @Override
    public void notifyApexScanResults(byte[] scanResults) {
        enforceSystemUid();
        com.android.server.pm.ApexBootFeed.notifyScanResults(scanResults);
    }

    private static void enforceSystemUid() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("the package bridge serves the system uid only");
        }
    }
}
