// Copyright (C) The Android Open Source Project, Apache License 2.0.
package dev.aim.server;

import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.storage.StorageManagerInternal;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;

/** aim-system's local views of the app host's existing owners (#2330). */
final class PackageLocalOwners {
    static IPackageShutdownBridge publish(ISystemProcess host, android.content.Context context,
            com.android.server.pm.Installer installer) {
        var users = android.os.IUserManager.Stub.asInterface(ServiceManager.getService("user"));
        LocalServices.addService(UserManagerInternal.class, new Users(java.util.Objects.requireNonNull(users)));
        LocalServices.addService(PermissionManagerServiceInternal.class, new Permissions(host));
        LocalServices.addService(StorageManagerInternal.class, new Storage(host));
        var packages = android.content.pm.IPackageManager.Stub.asInterface(ServiceManager.getService("package"));
        var dynamic = new com.android.server.pm.dex.DynamicCodeLogger(installer);
        var localPackages = new Packages(java.util.Objects.requireNonNull(packages), () -> {
            var service = IServiceHost.Stub.asInterface(java.util.Objects.requireNonNull(
                    ServiceManager.getService("aim.service_host"), "package snapshot owner absent"));
            return service.capturePackageScan();
        });
        localPackages.dynamic = dynamic;
        LocalServices.addService(android.content.pm.PackageManagerInternal.class, localPackages);
        var optimizer = new com.android.server.pm.PackageDexOptimizer(installer,
                new com.android.server.pm.PackageManagerTracedLock(), context, "*dexopt*");
        var dex = new com.android.server.pm.dex.DexManager(context, optimizer, dynamic);
        var installed = new java.util.HashMap<Integer, java.util.List<android.content.pm.PackageInfo>>();
        for (int user : LocalServices.getService(UserManagerInternal.class).getUserIds()) {
            installed.put(user, localPackages.installedPackages(user));
        }
        dex.load(installed);
        dynamic.load(installed);
        // getInstance constructs a Handler on its calling thread. Initialize on
        // aim-system's main Looper, before shutdown can arrive on a Binder thread.
        if (!android.crashrecovery.flags.Flags.refactorCrashrecovery())
            com.android.server.PackageWatchdog.getInstance(context);
        return new com.android.server.pm.NativePackageShutdownBridge(context, () -> {
            try { host.writePackageCompilerStats(); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }, dex, dynamic);
    }

    private static UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("outside package local adapter methods (#2330)");
    }

    static final class Users extends UserManagerInternal {
        private final android.os.IUserManager owner;
        Users(android.os.IUserManager owner) { this.owner = owner; }
        @Override public boolean exists(int user) {
            return getUserInfo(user) != null;
        }
        @Override public android.content.pm.UserInfo getUserInfo(int user) {
            try { return owner.getUserInfo(user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public android.content.pm.UserProperties getUserProperties(int user) {
            try { return exists(user) ? owner.getUserPropertiesCopy(user) : null; }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public boolean hasUserRestriction(String restriction, int user) {
            try { return owner.hasUserRestriction(restriction, user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public boolean isUserRunning(int user) {
            try { return owner.isUserRunning(user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public boolean isUserUnlockingOrUnlocked(int user) {
            try { return owner.isUserUnlockingOrUnlocked(user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public void addUserLifecycleListener(UserLifecycleListener l) { throw unsupported(); }
        @Override public void removeUserLifecycleListener(UserLifecycleListener l) { throw unsupported(); }
        @Override public void addUserVisibilityListener(UserVisibilityListener l) { throw unsupported(); }
        @Override public int[] getUserIds() {
            return getUsers(true, false, true).stream().mapToInt(user -> user.id).toArray();
        }
        @Override public java.util.List<android.content.pm.UserInfo> getUsers(boolean partial, boolean dying, boolean preCreated) {
            try { return owner.getUsers(partial, dying, preCreated); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public android.content.pm.UserInfo[] getUserInfos() {
            return getUsers(false, false, false).toArray(new android.content.pm.UserInfo[0]);
        }
        @Override public int getProfileParentId(int user) { throw unsupported(); }
        @Override public int[] getProfileIds(int user, boolean enabled) { throw unsupported(); }
    }

    /** Read slice needed by the original role helper and ProcessLoggingHandler. */
    static final class Packages extends NativePackageManagerInternal {
        private final android.content.pm.IPackageManager owner;
        private com.android.server.pm.dex.DynamicCodeLogger dynamic;
        @Override public com.android.server.pm.dex.DynamicCodeLogger getDynamicCodeLogger() {
            return java.util.Objects.requireNonNull(dynamic, "dynamic code owner unavailable");
        }
        private final PackageSnapshots.Source scans;
        private final ThreadLocal<PackageScanLease> iteration = new ThreadLocal<>();
        Packages(android.content.pm.IPackageManager owner, PackageSnapshots.Source scans) {
            this.owner = java.util.Objects.requireNonNull(owner);
            this.scans = java.util.Objects.requireNonNull(scans);
        }
        java.util.List<android.content.pm.PackageInfo> installedPackages(int user) {
            try { return owner.getInstalledPackages(0, user).getList(); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override @SuppressWarnings("rawtypes")
        public void requestChecksums(String name, boolean splits, int optional, int required,
                java.util.List trusted, android.content.pm.IOnChecksumsReadyListener listener,
                int user, java.util.concurrent.Executor executor, android.os.Handler handler) {
            java.util.Objects.requireNonNull(listener);
            java.util.Objects.requireNonNull(executor);
            java.util.Objects.requireNonNull(handler);
            try {
                owner.requestPackageChecksums(name, splits, optional, required, trusted,
                        new android.content.pm.IOnChecksumsReadyListener.Stub() {
                            @Override public void onChecksumsReady(java.util.List<android.content.pm.ApkChecksum> sums) {
                                executor.execute(() -> {
                                    try { listener.onChecksumsReady(sums); }
                                    catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
                                });
                            }
                        }, user);
            } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public void forEachInstalledPackage(
                java.util.function.Consumer<com.android.server.pm.pkg.AndroidPackage> action, int user) {
            java.util.Objects.requireNonNull(action);
            PackageScanLease previous = iteration.get();
            try {
                var endpoint = java.util.Objects.requireNonNull(scans.capture(), "package snapshot absent");
                try (var lease = new PackageScanLease(endpoint)) {
                    iteration.set(lease);
                    for (String name : endpoint.getPackageNames(false)) {
                        var state = lease.getUserState(name, false, user);
                        var code = lease.getCode(name, false);
                        if (code != null && state != null && state.installed)
                            action.accept(PackageObjects.fromSnapshot(code, lease.getVersion(), name));
                    }
                }
            } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
            catch (java.io.IOException e) { throw new IllegalStateException("package diagnostic capture failed", e); }
            finally { if (previous == null) iteration.remove(); else iteration.set(previous); }
        }
        private <T> T userState(String name, int user, java.util.function.Function<PackageUserStateData, T> read) {
            try {
                var current = iteration.get();
                if (current != null) return read.apply(current.getUserState(name, false, user));
                try (var lease = new PackageScanLease(java.util.Objects.requireNonNull(scans.capture()))) {
                    return read.apply(lease.getUserState(name, false, user));
                }
            } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
            catch (java.io.IOException e) { throw new IllegalStateException("package user capture failed", e); }
        }
        private static android.util.ArraySet<String> components(String[] names) {
            var result = new android.util.ArraySet<String>();
            if (names != null) java.util.Collections.addAll(result, names);
            return result;
        }
        @Override public int getApplicationEnabledState(String name, int user) {
            return userState(name, user, state -> state == null ? 0 : state.enabled);
        }
        @Override public android.util.ArraySet<String> getEnabledComponents(String name, int user) {
            return userState(name, user, state -> components(state == null ? null : state.getEnabledComponents()));
        }
        @Override public android.util.ArraySet<String> getDisabledComponents(String name, int user) {
            return userState(name, user, state -> components(state == null ? null : state.getDisabledComponents()));
        }
    }

    static final class Storage extends StorageManagerInternal {
        private final ISystemProcess owner;
        Storage(ISystemProcess owner) { this.owner = owner; }
        @Override public boolean isCeStoragePrepared(int user) {
            try { return owner.isPackageCeStoragePrepared(user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        // StorageManagerService.LocalService, android-16.0.0_r1 (Apache-2.0).
        @Override public void prepareAppDataAfterInstall(String name, int uid) {
            var vold = java.util.Objects.requireNonNull(android.os.IVold.Stub.asInterface(
                    ServiceManager.getService("vold")), "vold owner unavailable");
            for (var dir : new android.os.Environment.UserEnvironment(uid / 100000)
                    .buildExternalStorageAppObbDirs(name)) {
                if (dir.getPath().startsWith(android.os.Environment.getDataPreloadsMediaDirectory().getPath())) continue;
                try { vold.fixupAppDir(dir.getCanonicalPath() + "/", uid); }
                catch (java.io.IOException | RemoteException | android.os.ServiceSpecificException e) {
                    android.util.Slog.e("PackageManager", "Failed to fixup app dir for " + name, e);
                }
            }
        }
        @Override public void freeCache(String volume, long bytes) { throw unsupported(); }
        @Override public void prepareUserStorageForMove(String from, String to,
                java.util.List<android.content.pm.UserInfo> users) { throw unsupported(); }
    }

    static final class Permissions implements PermissionManagerServiceInternal {
        private final ISystemProcess owner;
        Permissions(ISystemProcess owner) { this.owner = owner; }
        @Override public void onPackageInstalled(com.android.server.pm.pkg.AndroidPackage pkg,
                int previous, PackageInstalledParams params, int user) {
            if (previous != -1) throw new IllegalArgumentException("existing permission previous app ID");
            try { owner.existingPackagePermissionsInstalled(pkg.getPackageName(),
                    params.getAllowlistedRestrictedPermissions().toArray(new String[0]), user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public void restoreDelayedRuntimePermissions(String name, int user) { throw unsupported(); }
        @Override public void resetRuntimePermissionsForUser(int user) {
            try { owner.resetPackageRuntimePermissionsForUser(user); }
            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
        }
        @Override public void resetRuntimePermissions(com.android.server.pm.pkg.AndroidPackage pkg, int user) { throw unsupported(); }
        @Override public java.util.List<android.content.pm.PermissionInfo> getAllPermissionsWithProtection(int protection) { throw unsupported(); }
        @Override public boolean isPermissionsReviewRequired(String name, int user) { throw unsupported(); }
        @Override public int[] getPermissionGids(String name, int user) { throw unsupported(); }
        @Override public java.util.Set<String> getInstalledPermissions(String name) { throw unsupported(); }
        @Override public java.util.Set<String> getGrantedPermissions(String name, int user) { throw unsupported(); }
        @Override public String getDefaultPermissionGrantFingerprint(int user) { throw unsupported(); }
        @Override public void setDefaultPermissionGrantFingerprint(String fingerprint, int user) { throw unsupported(); }
        @Override public void onSystemReady() { throw unsupported(); }
        @Override public void onUserCreated(int user) { throw unsupported(); }
        @Override public void onUserRemoved(int user) { throw unsupported(); }
        @Override public void readLegacyPermissionStateTEMP() { throw unsupported(); }
        @Override public void writeLegacyPermissionStateTEMP() { throw unsupported(); }
        @Override public int[] getGidsForUid(int uid) { throw unsupported(); }
        @Override public com.android.server.pm.permission.LegacyPermissionState getLegacyPermissionState(int id) { throw unsupported(); }
        @Override public HotwordDetectionServiceProvider getHotwordDetectionServiceProvider() { throw unsupported(); }
        @Override public void onPackageAdded(com.android.server.pm.pkg.PackageState state, boolean instant,
                com.android.server.pm.pkg.AndroidPackage old) { throw unsupported(); }
        @Override public void readLegacyPermissionsTEMP(com.android.server.pm.permission.LegacyPermissionSettings s) { throw unsupported(); }
        @Override public void writeLegacyPermissionsTEMP(com.android.server.pm.permission.LegacyPermissionSettings s) { throw unsupported(); }
        @Override public void onPackageRemoved(com.android.server.pm.pkg.AndroidPackage pkg) { throw unsupported(); }
        @Override public void onPackageUninstalled(String name, int id, com.android.server.pm.pkg.PackageState state,
                com.android.server.pm.pkg.AndroidPackage pkg, java.util.List<com.android.server.pm.pkg.AndroidPackage> shared, int user) { throw unsupported(); }
        @Override public void onStorageVolumeMounted(String volume, boolean changed) { throw unsupported(); }
    }
}
