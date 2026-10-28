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
    static void publish(ISystemProcess host) {
        var users = android.os.IUserManager.Stub.asInterface(ServiceManager.getService("user"));
        LocalServices.addService(UserManagerInternal.class, new Users(java.util.Objects.requireNonNull(users)));
        LocalServices.addService(PermissionManagerServiceInternal.class, new Permissions(host));
        LocalServices.addService(StorageManagerInternal.class, new Storage(host));
    }

    private static UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("outside package local adapter methods (#2330)");
    }

    static final class Users extends UserManagerInternal {
        private final android.os.IUserManager owner;
        Users(android.os.IUserManager owner) { this.owner = owner; }
        @Override public boolean exists(int user) {
            try { return owner.getUserInfo(user) != null; }
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
        @Override public int[] getUserIds() { throw unsupported(); }
        @Override public java.util.List<android.content.pm.UserInfo> getUsers(boolean a, boolean b, boolean c) { throw unsupported(); }
        @Override public android.content.pm.UserInfo[] getUserInfos() { throw unsupported(); }
        @Override public android.content.pm.UserInfo getUserInfo(int user) { throw unsupported(); }
        @Override public int getProfileParentId(int user) { throw unsupported(); }
        @Override public android.content.pm.UserProperties getUserProperties(int user) { throw unsupported(); }
        @Override public int[] getProfileIds(int user, boolean enabled) { throw unsupported(); }
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
        @Override public void resetRuntimePermissionsForUser(int user) { throw unsupported(); }
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
