package com.android.server.pm;

import android.content.Context;
import java.util.Objects;
import java.util.Set;

/** D2: original UserManagerService, with concrete PMS calls redirected at build time. */
public final class NativeUserManagerBridge {
    public interface Owner {
        void createNewUser(int userId, Set<String> installablePackages, String[] disallowedPackages);
        void onNewUserCreated(int userId, boolean convertedFromPreCreated);
        void cleanUpUser(UserManagerService userManager, int userId);
        Computer snapshotComputer();
        boolean hasSystemFeature(String name, int version);
        boolean isDeviceUpgrading();
        void addCrossProfileIntentFilter(Computer snapshot, WatchedIntentFilter filter,
                String ownerPackage, int sourceUserId, int targetUserId, int flags);
    }
    private static volatile Owner owner;
    private static volatile UserManagerService users;
    private static volatile UserManagerInternal internalUsers;

    /** Use only in the derived image whose fourteen UM sites passed the redirect verifier. */
    public static synchronized UserManagerService create(Context context, Installer installer,
            PackageManagerTracedLock installLock, Object packagesLock, Owner nativeOwner) {
        Objects.requireNonNull(context);
        Objects.requireNonNull(installer);
        Objects.requireNonNull(installLock);
        Objects.requireNonNull(packagesLock);
        Objects.requireNonNull(nativeOwner);
        if (owner != null) throw new IllegalStateException("native user package owner already attached");
        owner = nativeOwner;
        try {
            UserManagerService service = new UserManagerService(context, null,
                    new UserDataPreparer(installer, installLock, context), packagesLock);
            users = service;
            internalUsers = com.android.server.LocalServices.getService(UserManagerInternal.class);
            if (internalUsers == null) throw new IllegalStateException("original user internal owner unavailable");
            return service;
        } catch (RuntimeException | Error failure) {
            owner = null;
            throw failure;
        }
    }
    /** Actual UM stage in PMS.systemReady, before storage listeners/installer readiness. */
    public static void systemReady(UserManagerService service) {
        requireUsers(service).systemReady();
    }
    /** Actual UM reconciliation after installer and dex-optimizer readiness. */
    public static void reconcileUsers(UserManagerService service, String volumeUuid) {
        requireUsers(service).reconcileUsers(volumeUuid);
    }
    private static UserManagerService requireUsers(UserManagerService service) {
        if (service == null || service != users) throw new IllegalArgumentException("foreign user lifecycle owner");
        return service;
    }
    private static Owner owner(PackageManagerService original) {
        if (original != null) throw new IllegalStateException("native user bridge received original PMS");
        Owner current = owner;
        if (current == null) throw new IllegalStateException("native user package owner unavailable");
        return current;
    }
    public static void createNewUser(PackageManagerService original, int userId,
            Set<String> installablePackages, String[] disallowedPackages) {
        owner(original).createNewUser(userId, installablePackages, disallowedPackages);
    }
    public static void onNewUserCreated(PackageManagerService original, int userId,
            boolean convertedFromPreCreated) {
        owner(original).onNewUserCreated(userId, convertedFromPreCreated);
    }
    public static void cleanUpUser(PackageManagerService original,
            UserManagerService userManager, int userId) {
        owner(original).cleanUpUser(userManager, userId);
    }
    public static Computer snapshotComputer(PackageManagerService original) {
        return Objects.requireNonNull(owner(original).snapshotComputer(), "missing native Computer");
    }
    public static boolean hasSystemFeature(PackageManagerService original, String name, int version) {
        return owner(original).hasSystemFeature(name, version);
    }
    public static boolean isDeviceUpgrading(PackageManagerService original) {
        return owner(original).isDeviceUpgrading();
    }
    public static void addCrossProfileIntentFilter(PackageManagerService original,
            Computer snapshot, WatchedIntentFilter filter, String ownerPackage,
            int sourceUserId, int targetUserId, int flags) {
        owner(original).addCrossProfileIntentFilter(snapshot, filter, ownerPackage,
                sourceUserId, targetUserId, flags);
    }
    private NativeUserManagerBridge() {}
    /** PMS getUsers(false): original UM excludes partial and pre-created users. */
    public static java.util.List<android.content.pm.UserInfo> getUsers() {
        UserManagerService service = users;
        if (service == null) throw new IllegalStateException("native user owner unavailable");
        long token = android.os.Binder.clearCallingIdentity();
        try { return service.getUsers(false); }
        finally { android.os.Binder.restoreCallingIdentity(token); }
    }
    public static android.content.pm.UserInfo getProfileParent(int userId) {
        UserManagerService service = users;
        if (service == null) throw new IllegalStateException("native user owner unavailable");
        long token = android.os.Binder.clearCallingIdentity();
        try { return service.getProfileParent(userId); }
        finally { android.os.Binder.restoreCallingIdentity(token); }
    }
    /** Computer.getUserInfos uses the complete original UserManagerInternal records. */
    public static android.content.pm.UserInfo[] getUserInfos() {
        UserManagerInternal current = internalUsers;
        if (users == null || current == null
                || com.android.server.LocalServices.getService(UserManagerInternal.class) != current)
            throw new IllegalStateException("original user internal owner unavailable or replaced");
        return current.getUserInfos();
    }

    private static android.content.Context systemContext() {
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return thread.getSystemContext();
    }

    private static com.android.server.pm.permission.PermissionManagerServiceInternal permissions() {
        var current = com.android.server.LocalServices.getService(
                com.android.server.pm.permission.PermissionManagerServiceInternal.class);
        if (current == null) throw new IllegalStateException("permission owner unavailable");
        return current;
    }

    public static boolean isPermissionsReviewRequired(String packageName, int userId) {
        return permissions().isPermissionsReviewRequired(packageName, userId);
    }

    public static int[] getPermissionGids(String permissionName, int userId) {
        return permissions().getPermissionGids(permissionName, userId);
    }

    public static String getDefaultHome(int userId) {
        var current = com.android.server.LocalServices.getService(UserManagerInternal.class);
        if (current == null) throw new IllegalStateException("user profile owner unavailable");
        var roles = systemContext().getSystemService(android.app.role.RoleManager.class);
        if (roles == null) return null;
        long token = android.os.Binder.clearCallingIdentity();
        try {
            var holders = roles.getRoleHoldersAsUser("android.app.role.HOME",
                    android.os.UserHandle.of(current.getProfileParentId(userId)));
            return holders.isEmpty() ? null : holders.get(0);
        } finally { android.os.Binder.restoreCallingIdentity(token); }
    }

    public static android.content.ComponentName getSystemUiServiceComponent() {
        String service = systemContext().getResources().getString(
                com.android.internal.R.string.config_systemUIServiceComponent);
        return android.content.ComponentName.unflattenFromString(service);
    }
    private static UserManagerInternal requireUserInternal() {
        var current=internalUsers;
        if(current==null||com.android.server.LocalServices.getService(UserManagerInternal.class)!=current)
            throw new IllegalStateException("original user owner unavailable or replaced");
        return current;
    }
    public static void enforceShellRestriction(int uid,int user) {
        PackageManagerServiceUtils.enforceShellRestriction(requireUserInternal(),
                android.os.UserManager.DISALLOW_DEBUGGING_FEATURES,uid,user);
    }
    public static boolean isSameProfileGroup(int first,int second) {
        UserManagerService current=users;
        if(current==null||UserManagerService.getInstance()!=current)
            throw new IllegalStateException("original user service unavailable or replaced");
        long identity=android.os.Binder.clearCallingIdentity();
        try{return current.isSameProfileGroup(first,second);}
        finally{android.os.Binder.restoreCallingIdentity(identity);}
    }
    public static boolean hasCrossProfilePermission(int uid,String packageName) {
        return android.content.PermissionChecker.checkPermissionForPreflight(systemContext(),
                "android.permission.INTERACT_ACROSS_PROFILES",android.content.PermissionChecker.PID_UNKNOWN,
                uid,packageName)==android.content.PermissionChecker.PERMISSION_GRANTED;
    }
    public static boolean isKnownIsolatedComputeApp(int uid) {
        if(!android.os.Process.isIsolatedUid(uid))return false;
        var hotword=permissions().getHotwordDetectionServiceProvider();
        if(hotword!=null&&hotword.getUid()==uid)return true;
        var inference=com.android.server.LocalManagerRegistry.getManager(
                com.android.server.ondeviceintelligence.OnDeviceIntelligenceManagerLocal.class);
        return inference!=null&&inference.getInferenceServiceUid()==uid;
    }
    public static String resolveTypeIfNeeded(android.content.Intent intent) {
        return intent.resolveTypeIfNeeded(systemContext().getContentResolver());
    }

}
