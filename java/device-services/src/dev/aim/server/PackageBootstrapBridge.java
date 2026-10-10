package dev.aim.server;
import java.util.Objects;
import android.os.UserHandle;

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
    private IPackageShellReadLeaf packageShellRead;
    public synchronized void installPackageShellReadOwner(Context context,PackageSnapshots.Store snapshots){
        if(packageShellRead!=null)throw new IllegalStateException("package shell read owner already installed");
        packageShellRead=new NativePackageShellReadLeaf(context,snapshots);
    }
    @Override public synchronized IPackageShellReadLeaf getPackageShellReadLeaf(){
        enforceSystemUid();return Objects.requireNonNull(packageShellRead,"full native shell read owner unavailable");
    }
    @Override public IPackageShellInstallPolicy getPackageShellInstallPolicy(){
        enforceSystemUid();return new PackageShellInstallPolicy();
    }

    @Override public IPackageShellPolicyBridge getPackageShellPolicyBridge() {
        enforceSystemUid();return new PackageShellPolicyBridge(systemContext());
    }
    private com.android.server.pm.NativeDomainSettings domainSettings;
    public synchronized void installPackageDomainSettings(
            java.util.function.Supplier<com.android.server.pm.NativeComputer> computers,
            android.os.Handler handler) {
        enforceSystemUid();
        if (domainSettings != null) throw new IllegalStateException("domain settings owner already installed");
        var original = (com.android.server.pm.verify.domain.DomainVerificationService) domains;
        domainSettings = new com.android.server.pm.NativeDomainSettings(original, computers, handler);
        var connection = new com.android.server.pm.NativeDomainVerificationConnection(
                original, computers, java.util.Objects.requireNonNull(com.android.server.LocalServices.getService(
                        com.android.server.pm.UserManagerInternal.class)), domainSettings, handler);
        original.setConnection(connection);
        connection.installProxy(systemContext());
    }
    @Override public synchronized IPackageDomainSettings getPackageDomainSettings() {
        enforceSystemUid();
        if (domainSettings == null) throw new IllegalStateException("domain settings owner unavailable");
        return domainSettings;
    }
    private final DomainVerificationManagerInternal domains;
    private final com.android.server.pm.Installer existingInstaller;
    private IPackageInstallerFiles installerFiles;
    private INativeStagingBridge stagingBridge;
    private PackagePolicyBridge packagePolicy;
    private InstallerExternalBridge installerExternal;
    private IPackageMaintenanceBridge packageMaintenance;
    private com.android.server.pm.NativePackageEffectsBridge packageEffects;
    private PackageMoveBridge packageMoves;
    private PackageRelocationBridge packageRelocation;
    private com.android.server.pm.NativeApplicationDataBridge applicationData;
    private IPackageAppDataBridge packageAppData;
    private IInstallerPermissionBridge installerPermissions;
    private IInstallerPreparationBridge installerPreparation;
    private IInstallerArchiveQueryBridge installerArchiveQueries;
    private IPackageInternalStorageBridge internalStorage;
    private IPackageShutdownBridge packageShutdown;
    private com.android.server.pm.NativePackageObserverOwner packageObservers;

    public PackageBootstrapBridge() { this(null); }
    public PackageBootstrapBridge(DomainVerificationManagerInternal domains) {
        this(domains, null);
    }
    public PackageBootstrapBridge(DomainVerificationManagerInternal domains, com.android.server.pm.Installer installer) {
        this.domains = domains;
        this.existingInstaller = installer;
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
            attach(domains, installer);
        } catch (RemoteException failure) {
            throw new IllegalStateException("package bootstrap attach failed", failure);
        }
        return com.android.server.pm.PackageManagerService.main(context, installer, domains, factoryTest);
    }

    /** Called by the C facade before native scanning, after PlatformCompat starts. */
    public static void attach(DomainVerificationManagerInternal domains) throws RemoteException {
        attach(domains, null);
    }
    public static void attach(DomainVerificationManagerInternal domains, com.android.server.pm.Installer installer) throws RemoteException {
        attachOwner(domains, installer);
    }
    public static PackageBootstrapBridge attachOwner(DomainVerificationManagerInternal domains, com.android.server.pm.Installer installer) throws RemoteException {
        if (domains == null) throw new IllegalArgumentException("missing domain owner");
        IBinder host = ServiceManager.checkService("aim.service_host");
        if (host == null) throw new IllegalStateException("native service host is unavailable");
        PackageBootstrapBridge owner = new PackageBootstrapBridge(domains, installer);
        IServiceHost.Stub.asInterface(host).attachPackageBootstrapBridge(owner);
        return owner;
    }

    /** Capture a complete native replica before exposing the facade's scopes. */
    public static PackageSnapshots.Store captureSnapshots(PackageSnapshots.Owner owner,
            boolean crossUserSuspensions) throws RemoteException, java.io.IOException {
        IBinder binder = ServiceManager.checkService("aim.service_host");
        if (binder == null) throw new IllegalStateException("native service host is unavailable");
        var host = IServiceHost.Stub.asInterface(binder);
        var versions = new PackageVersionPage(host.getPackageStateVersionPage());
        var snapshots = new PackageSnapshots.Store(host::capturePackageScan, owner, crossUserSuspensions, versions);
        try { snapshots.refresh(); }
        catch (RemoteException | java.io.IOException | RuntimeException failure) { versions.close(); throw failure; }
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
        var versions = new PackageVersionPage(host.getPackageStateVersionPage());
        var snapshots = new PackageSnapshots.Store(host::capturePackageScan, owner, crossUserSuspensions, versions);
        try { snapshots.refresh(); }
        catch (RemoteException | java.io.IOException | RuntimeException failure) { versions.close(); throw failure; }
        return new PackageLocal(snapshots, sdkDataOwner(host), signingOwner(host));
    }

    @Override
    public void restoreInstallerContext(String path) {
        enforceSystemUid();
        if (path == null || !(path.startsWith("/data/app/")
                || path.startsWith("/data/app-staging/")
                || (path.equals("/data/system/install_sessions.xml")
                    || path.equals("/data/system/install_sessions.xml.new")
                    || path.equals("/data/system/install_sessions.xml.bak")))
                || path.contains("/../") || path.contains("/./")
                || path.endsWith("/..") || path.endsWith("/."))
            throw new IllegalArgumentException("invalid installer context path");
        if (!android.os.SELinux.restorecon(new java.io.File(path)))
            throw new IllegalStateException("installer restorecon failed: " + path);
    }

    @Override
    public byte[] getInstallerUserPolicy(int userId) {
        enforceSystemUid();
        return InstallerUserPolicy.capture(userId);
    }

    @Override
    public void allocateInstallerBytes(android.os.ParcelFileDescriptor file, long lengthBytes, int installFlags) {
        enforceSystemUid();
        if (file == null || lengthBytes <= 0) throw new IllegalArgumentException("invalid installer allocation");
        try (file) {
            var thread = android.app.ActivityThread.currentActivityThread();
            if (thread == null) throw new IllegalStateException("system context owner is unavailable");
            Context context = thread.getSystemContext();
            var storage = context.getSystemService(android.os.storage.StorageManager.class);
            if (storage == null) throw new IllegalStateException("storage allocation owner is unavailable");
            storage.allocateBytes(file.getFileDescriptor(), lengthBytes,
                    com.android.internal.content.InstallLocationUtils.translateAllocateFlags(installFlags));
        } catch (java.io.IOException failure) {
            throw new android.os.ParcelableException(failure);
        }
    }

    @Override
    public boolean isInstallerRevocableFdEnabled() {
        enforceSystemUid();
        return android.content.pm.PackageInstaller.ENABLE_REVOCABLE_FD;
    }

    @Override
    public long[] getInstallerDomainLimits() {
        enforceSystemUid();
        String namespace = android.provider.DeviceConfig.NAMESPACE_PACKAGE_MANAGER_SERVICE;
        return new long[] {
            android.provider.DeviceConfig.getLong(namespace, "pre_verified_domains_count_limit", 1000),
            android.provider.DeviceConfig.getLong(namespace, "pre_verified_domain_length_limit", 256)
        };
    }

    @Override
    public boolean installerCloudCompilationVerificationEnabled() {
        enforceSystemUid();
        return com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.cloudCompilationVerification();
    }

    @Override
    public boolean isInstallerArtServiceV3Enabled() {
        enforceSystemUid();
        return com.android.art.flags.Flags.artServiceV3();
    }

    @Override
    public IPackageResolverIdentity allocatePreferredResolverRecord(byte[] record) {
        enforceSystemUid();
        return com.android.server.pm.NativePreferredRecords.allocate(record);
    }

    @Override
    public IInstallerConfirmationBridge getInstallerConfirmationBridge(boolean dependencyInstallerEnabled) {
        enforceSystemUid();
        return new InstallerConfirmationBridge(Objects.requireNonNull(packagePolicy), dependencyInstallerEnabled);
    }

    @Override
    public IPackageLifecycleLeaf getPackageLifecycleLeaf() {
        enforceSystemUid();
        return new PackageLifecycleLeaf();
    }

    @Override
    public IPackageEnableBridge getPackageEnableBridge() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new PackageEnableBridge(thread.getSystemContext());
    }

    @Override
    public synchronized IPackageAppDataBridge getPackageAppDataBridge() {
        enforceSystemUid();
        return Objects.requireNonNull(packageAppData, "native app-data producer unavailable");
    }

    public synchronized void installInstallerPermissionOwner(PackageSnapshots.Store packages,
            PackageSnapshots.Owner owner, boolean crossUserSuspensions) {
        if (installerPermissions != null) throw new IllegalStateException("installer permission owner already attached");
        installerPermissions = new InstallerPermissionBridge(
                new NativeInstallPermissionScope(packages, owner, crossUserSuspensions));
    }

    @Override
    public synchronized IInstallerPermissionBridge getInstallerPermissionBridge() {
        enforceSystemUid();
        return Objects.requireNonNull(installerPermissions, "installer permission producer unavailable");
    }

    @Override
    public synchronized IInstallerPreparationBridge getInstallerPreparationBridge() {
        enforceSystemUid();
        if (installerPreparation == null) {
            var thread = android.app.ActivityThread.currentActivityThread();
            if (thread == null) throw new IllegalStateException("system context unavailable");
            installerPreparation = new com.android.server.pm.NativeInstallerPreparationBridge(thread.getSystemContext());
        }
        return installerPreparation;
    }

    @Override
    public synchronized IBinder getInstallerRemovalBridge() {
        enforceSystemUid();
        getInstallerExternalBridge();
        return installerExternal.getRemovalBridge();
    }

    @Override
    public synchronized IInstallerArchiveQueryBridge getInstallerArchiveQueryBridge() {
        enforceSystemUid();
        if (installerArchiveQueries == null) {
            var thread = android.app.ActivityThread.currentActivityThread();
            if (thread == null) throw new IllegalStateException("system context unavailable");
            installerArchiveQueries = new InstallerArchiveQueryBridge(thread.getSystemContext());
        }
        return installerArchiveQueries;
    }

    @Override
    public IBinder getPackageLaunchBridge() {
        enforceSystemUid();
        return new PackageLaunchBridge().asBinder();
    }

    @Override
    public IInstallerPolicyBridge getInstallerPolicyBridge() {
        enforceSystemUid();
        return new com.android.server.pm.NativeInstallerPolicyBridge();
    }

    @Override
    public IInstallerRecoveryPresentation getInstallerRecoveryPresentation() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new InstallerRecoveryPresentation(thread.getSystemContext());
    }

    @Override
    public synchronized IPackageInternalStorageBridge getPackageInternalStorageBridge() {
        enforceSystemUid();
        return Objects.requireNonNull(internalStorage, "native internal storage producer unavailable");
    }

    @Override
    public IPackageApexUninstallBridge getPackageApexUninstallBridge() {
        enforceSystemUid();
        return new com.android.server.pm.NativePackageApexUninstallBridge();
    }

    @Override
    public IPermissionPersistenceBridge getPermissionPersistenceBridge() {
        enforceSystemUid();
        return new PermissionPersistenceBridge();
    }

    @Override
    public IPackageDiagnosticInputs getPackageDiagnosticInputs() {
        enforceSystemUid();
        return new PackageDiagnosticInputs(systemContext());
    }

    @Override
    public IPackageInternalEventsBridge getPackageInternalEventsBridge() {
        enforceSystemUid();
        return new PackageInternalEventsBridge();
    }

    public synchronized void installPackageShutdownOwner(com.android.server.pm.CompilerStats compiler,
            com.android.server.pm.dex.DexManager dex,
            com.android.server.pm.dex.DynamicCodeLogger dynamicCode, Context context) {
        if (packageShutdown != null) throw new IllegalStateException("package shutdown owner already attached");
        packageShutdown = new com.android.server.pm.NativePackageShutdownBridge(context, compiler, dex, dynamicCode);
    }

    @Override
    public synchronized IPackageShutdownBridge getPackageShutdownBridge() {
        enforceSystemUid();
        return Objects.requireNonNull(packageShutdown, "package shutdown producer unavailable");
    }

    public synchronized void installPackageObserverOwner(com.android.server.pm.NativePackageObserverOwner owner) {
        if (packageObservers != null) throw new IllegalStateException("package observer owner already attached");
        packageObservers = Objects.requireNonNull(owner);
    }

    @Override
    public synchronized IPackageObserverEventsBridge getPackageObserverEventsBridge() {
        enforceSystemUid();
        return Objects.requireNonNull(packageObservers, "package observer producer unavailable");
    }

    @Override
    public IPackageBootContextLeaf getPackageBootContextLeaf() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new com.android.server.pm.NativePackageBootContextLeaf(thread.getSystemContext());
    }

    @Override
    public IPackageInitialContextLeaf getPackageInitialContextLeaf() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new PackageInitialContextLeaf(thread.getSystemContext());
    }

    @Override
    public IPackageBootLifecycleLeaf getPackageBootLifecycleLeaf() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new com.android.server.pm.NativePackageBootLifecycleLeaf(thread.getSystemContext(),
                com.android.internal.os.BackgroundThread.getHandler());
    }

    @Override
    public IWebInstantAppsState getWebInstantAppsState() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new WebInstantAppsState(thread.getSystemContext(), com.android.internal.os.BackgroundThread.getHandler());
    }

    @Override
    public IInstallerCompletionBridge getInstallerCompletionBridge() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new InstallerCompletionBridge(thread.getSystemContext());
    }

    @Override
    public IPackageBootConfigurationLeaf getPackageBootConfigurationLeaf(boolean factoryTest) {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new PackageBootConfigurationLeaf(thread.getSystemContext(), factoryTest);
    }

    @Override
    public byte[] packageMonitorResult(String action, String packageName, int userId, int uid, boolean replacing) {
        enforceSystemUid();
        android.os.Bundle extras = new android.os.Bundle();
        extras.putInt(android.content.Intent.EXTRA_UID, uid);
        extras.putBoolean(android.content.Intent.EXTRA_REPLACING, replacing);
        return NativePackageMonitorPayload.serialize(action, packageName, extras, userId);
    }

    @Override
    public IPackageResolverIdentity allocatePreferredResolverIdentity() {
        enforceSystemUid();
        return new IPackageResolverIdentity.Stub() {
            @Override
            public int getIdentityHash() {
                enforceSystemUid();
                return System.identityHashCode(this);
            }
        };
    }

    @Override
    public synchronized IBinder getPackageInstallerFiles() {
        enforceSystemUid();
        if (installerFiles == null) {
            installerFiles = com.android.server.pm.PackageInstallerFileBridge.create();
        }
        return installerFiles.asBinder();
    }

    @Override
    public void invalidatePackagesForUidCache() {
        enforceSystemUid();
        android.app.ApplicationPackageManager.invalidateGetPackagesForUidCache();
    }

    @Override
    public synchronized IBinder getNativeStagingBridge() {
        enforceSystemUid();
        if (stagingBridge == null) {
            stagingBridge = new com.android.server.pm.NativeStagingBridge();
        }
        return stagingBridge.asBinder();
    }

    @Override
    public synchronized IBinder getPackagePolicyBridge() {
        enforceSystemUid();
        if (packagePolicy == null) {
            var thread = android.app.ActivityThread.currentActivityThread();
            if (thread == null) throw new IllegalStateException("system context unavailable");
            packagePolicy = new PackagePolicyBridge(thread.getSystemContext());
        }
        return packagePolicy.asBinder();
    }

    @Override
    public boolean isAutoRevokeWhitelisted(int callingUid, String packageName) {
        enforceSystemUid();
        return NativePermissionQueries.isAutoRevokeWhitelisted(callingUid, packageName);
    }

    @Override
    public synchronized IBinder getInstallerExternalBridge() {
        enforceSystemUid();
        if (installerExternal == null) {
            var thread = android.app.ActivityThread.currentActivityThread();
            if (thread == null) throw new IllegalStateException("system context unavailable");
            installerExternal = new InstallerExternalBridge(thread.getSystemContext());
        }
        return installerExternal.asBinder();
    }

    public synchronized void installPackageMaintenanceOwner(PackageSnapshots.Store packages,
            android.content.pm.dex.IArtManager art, Context context,
            com.android.server.pm.Installer installer,
            com.android.server.pm.PackageManagerTracedLock installLock, java.io.File parserCache) {
        if (packageMaintenance != null) throw new IllegalStateException("maintenance owner already attached");
        packageMaintenance = new com.android.server.pm.NativePackageMaintenanceBridge(
                packages, art, context, installer, installLock, parserCache);
        internalStorage = new com.android.server.pm.NativePackageInternalStorageBridge(context, installer, packages);
        var installd = android.os.IInstalld.Stub.asInterface(Objects.requireNonNull(
                android.os.ServiceManager.checkService("installd"), "installd owner unavailable"));
        packageAppData = new com.android.server.pm.NativePackageAppDataBridge(installd,
                Objects.requireNonNull(com.android.server.LocalServices.getService(com.android.server.pm.UserManagerInternal.class)),
                () -> com.android.server.LocalServices.getService(android.os.storage.StorageManagerInternal.class),
                installLock);
    }

    @Override
    public synchronized IBinder getPackageMaintenanceBridge() {
        enforceSystemUid();
        if (packageMaintenance == null) throw new IllegalStateException("native maintenance producer unavailable");
        return packageMaintenance.asBinder();
    }

    public synchronized void installPackageMutationOwner(Context context, android.os.Handler handler,
            java.util.function.Supplier<com.android.server.pm.Computer> snapshots,
            com.android.server.pm.NativePackageEffectsBridge.Policy policy) {
        if (packageEffects != null) throw new IllegalStateException("package effects already attached");
        getPackagePolicyBridge();
        packageEffects = new com.android.server.pm.NativePackageEffectsBridge(context, handler,
                snapshots, policy, packagePolicy.protectedPackages());
    }

    @Override
    public synchronized IBinder getPackageMutationBridge() {
        enforceSystemUid();
        if (packageEffects == null) throw new IllegalStateException("native package effects producer unavailable");
        return packageEffects.asBinder();
    }

    @Override
    public synchronized IBinder getPackageMoveBridge() {
        enforceSystemUid();
        if (packageMoves == null) packageMoves = new PackageMoveBridge(systemContext());
        return packageMoves.asBinder();
    }

    @Override
    public synchronized IBinder getPackageRelocationBridge() {
        enforceSystemUid();
        if (packageRelocation == null) packageRelocation = new PackageRelocationBridge(systemContext());
        return packageRelocation.asBinder();
    }

    @Override
    public synchronized IBinder getApplicationDataBridge() {
        enforceSystemUid();
        if (packageMaintenance == null) throw new IllegalStateException("native maintenance producer unavailable");
        if (applicationData == null) applicationData = new com.android.server.pm.NativeApplicationDataBridge(
                systemContext(), packageMaintenance);
        return applicationData.asBinder();
    }

    @Override
    public boolean checkProviderAuthorityGrants(int callingUid,
            android.content.pm.ProviderInfo providerInfo, int userId) {
        enforceSystemUid();
        var grants = LocalServices.getService(com.android.server.uri.UriGrantsManagerInternal.class);
        if (grants == null) throw new IllegalStateException("URI grants owner unavailable");
        return grants.checkAuthorityGrants(callingUid, providerInfo, userId, true);
    }

    @Override
    public boolean isProviderCloneRedirected(String authority, int callingUid, int userId) {
        enforceSystemUid();
        if (!android.content.ContentProvider.isAuthorityRedirectedForCloneProfile(authority)) return false;
        var users = LocalServices.getService(com.android.server.pm.UserManagerInternal.class);
        if (users == null) throw new IllegalStateException("user clone owner unavailable");
        var info = users.getUserInfo(android.os.UserHandle.getUserId(callingUid));
        return info != null && info.isCloneProfile() && info.profileGroupId == userId;
    }

    private static Context systemContext() {
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return thread.getSystemContext();
    }

    @Override
    public int getInstantAppCookieLimit() {
        enforceSystemUid();
        return android.provider.Settings.Global.getInt(systemContext().getContentResolver(),
                "ephemeral_cookie_max_size_bytes", 16384);
    }

    @Override
    public int getInstantAppIconDensity() {
        enforceSystemUid();
        var bitmap = android.graphics.Bitmap.createBitmap(1, 1, android.graphics.Bitmap.Config.ARGB_8888);
        try {
            return bitmap.getDensity();
        } finally {
            bitmap.recycle();
        }
    }

    @Override
    public int getPackageProfileParent(int userId) {
        enforceSystemUid();
        var users = com.android.server.pm.UserManagerService.getInstance();
        if (users == null) throw new IllegalStateException("user parent owner unavailable");
        long token = Binder.clearCallingIdentity();
        try {
            var parent = users.getProfileParent(userId);
            return parent == null ? -10000 : parent.id;
        } finally { Binder.restoreCallingIdentity(token); }
    }

    @Override
    public boolean isParentProfileAppLinkingAllowed(int userId) {
        enforceSystemUid();
        var users = LocalServices.getService(com.android.server.pm.UserManagerInternal.class);
        if (users == null) throw new IllegalStateException("user linking owner unavailable");
        return users.hasUserRestriction("allow_parent_profile_app_linking", userId);
    }

    @Override
    public String[] getPackageRoleHolders(String role, int userId) {
        enforceSystemUid();
        var roles = systemContext().getSystemService(android.app.role.RoleManager.class);
        if (roles == null) throw new IllegalStateException("role owner unavailable");
        long token = Binder.clearCallingIdentity();
        try { return roles.getRoleHoldersAsUser(role, UserHandle.of(userId)).toArray(new String[0]); }
        finally { Binder.restoreCallingIdentity(token); }
    }

    @Override
    public int packageMonitorUser(int callingPid, int callingUid, int userId) {
        enforceSystemUid();
        return android.app.ActivityManager.handleIncomingUser(callingPid, callingUid, userId,
                true, true, "registerPackageMonitorCallback", "android");
    }

    @Override
    public boolean waitPackageBackgroundHandler(long timeoutMillis) {
        enforceSystemUid();
        return NativePackageHandlers.waitForBackground(timeoutMillis);
    }

    @Override
    public void logPackageProcessStart(String packageName, String processName, int uid,
            String seinfo, String apkFile, int pid) {
        enforceSystemUid();
        com.android.server.pm.NativeProcessLoggingBridge.log(packageName, processName,
                uid, seinfo, apkFile, pid);
    }

    @Override
    public boolean isShellDebuggingRestricted(int userId) {
        enforceSystemUid();
        return InstallerUserPolicy.shellDebuggingRestricted(userId);
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
    public boolean isSdkLibraryIndependenceEnabled() {
        enforceSystemUid();
        return com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.sdkLibIndependence();
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

    @Override
    public boolean isDomainSetUuidStrictValidationEnabled() {
        enforceSystemUid();
        // UUID.ENABLE_STRICT_VALIDATION, android-16.0.0_r1.
        return dalvik.system.VMRuntime.getSdkVersion() >= 34
                && android.compat.Compatibility.isChangeEnabled(263076149L);
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

    @Override
    public byte[] getCurrentPackageVersion() {
        enforceSystemUid();
        return com.android.server.pm.PackageBootVersion.capture();
    }

    private static void enforceSystemUid() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("the package bridge serves the system uid only");
        }
    }
    @Override public long[] onExistingPackageInstalled(android.os.IBinder record, int userId, int installFlags) {
        enforceSystemUid();
        try { return ExistingInstallEffects.installed(existingInstaller, record, userId, installFlags); }
        catch (RuntimeException failure) { throw failure; }
        catch (Exception failure) { throw new IllegalStateException("existing install effects failed", failure); }
    }
    @Override public boolean restoreExistingPackageInstall(String name, int userId, int token) throws RemoteException {
        enforceSystemUid(); return ExistingInstallEffects.restore(name, userId, token);
    }
    @Override public String completeExistingPackageInstall(String name, int userId, android.content.IntentSender target, int status, boolean restorePermissions) {
        enforceSystemUid();
        try { return ExistingInstallEffects.complete(name, userId, target, status, restorePermissions); }
        catch (RuntimeException failure) { throw failure; }
        catch (Exception failure) { throw new IllegalStateException("existing install completion failed", failure); }
    }
    @Override public byte[] getExistingPackageInstallUserPolicy(int userId) {
        enforceSystemUid(); return ExistingInstallEffects.userPolicy(userId);
    }
    @Override public int getPreferredCrossProfileAccessControl(int source, int target) {
        enforceSystemUid(); return PreferredPolicyBridge.crossProfileAccess(source, target);
    }
    @Override public String getPreferredRoleHolder(String role, int user) {
        enforceSystemUid(); return PreferredPolicyBridge.roleHolder(role, user);
    }
    @Override public void setPreferredRoleHolder(String role, String name, int user, boolean broadcast) {
        enforceSystemUid(); PreferredPolicyBridge.roleHolder(role, name, user, broadcast);
    }
    @Override public void sendPreferredActivityChanged(int user) {
        enforceSystemUid(); PreferredPolicyBridge.preferredChanged(user);
    }
    @Override public void resetPreferredRuntimePermissions(int user) {
        enforceSystemUid(); PreferredPolicyBridge.resetPermissions(user);
    }
    @Override public void resetPreferredNetworkPolicies(int user) {
        enforceSystemUid(); PreferredPolicyBridge.resetNetwork(user);
    }
    @Override public IPackageResolutionPolicy getPackageResolutionPolicy() {
        enforceSystemUid();
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new com.android.server.pm.NativePackageResolutionPolicy(thread.getSystemContext(),
                java.util.Objects.requireNonNull(com.android.server.LocalServices.getService(com.android.server.pm.UserManagerInternal.class)));
    }
    @Override public String formatPackageTimestamp(long millis) {
        enforceSystemUid();
        java.text.SimpleDateFormat format = new java.text.SimpleDateFormat("yyyy-MM-dd HH:mm:ss");
        return format.format(new java.util.Date(millis));
    }
}
