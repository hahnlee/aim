package com.android.server.pm;

import android.content.Context;
import android.os.Binder;
import android.os.IInstalld;
import android.os.ServiceManager;
import com.android.server.LocalServices;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import dev.aim.server.IApplicationDataBridge;
import dev.aim.server.IPackageMaintenanceBridge;
import java.util.Objects;

/** Original independent data, permission, keystore and AM owners; no original PMS. */
public final class NativeApplicationDataBridge extends IApplicationDataBridge.Stub {
    private final Context context;
    private final IPackageMaintenanceBridge maintenance;
    public NativeApplicationDataBridge(Context context, IPackageMaintenanceBridge maintenance) {
        this.context = Objects.requireNonNull(context); this.maintenance = Objects.requireNonNull(maintenance);
    }
    private static void enforce() {
        int uid = Binder.getCallingUid();
        if (uid != 0 && uid != 1000) throw new SecurityException("untrusted application data owner caller");
    }
    @Override public void enforcePermission(String permission, int pid, int uid) {
        enforce(); context.enforcePermission(permission, pid, uid, permission);
    }
    @Override public void killAndWait(String name, int appId) {
        enforce();
        var am = Objects.requireNonNull(LocalServices.getService(android.app.ActivityManagerInternal.class));
        if (com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.waitApplicationKilled()) {
            KillAppBlocker blocker = new KillAppBlocker();
            try {
                blocker.register(); am.killApplicationSync(name, appId, -1, "clearApplicationUserData", 10);
                blocker.waitAppProcessGone(am, NativeUserManagerBridge.snapshotComputer(null), UserManagerService.getInstance(), name);
            } finally { blocker.unregister(); }
        } else {
            try { android.app.ActivityManager.getService().killApplication(name, appId, -1, "clearApplicationUserData", 10); }
            catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
    }
    @Override public boolean clearData(String name, int user) throws android.os.RemoteException {
        enforce();
        Computer snapshot = NativeUserManagerBridge.snapshotComputer(null);
        var pkg = name == null ? null : snapshot.getPackage(name);
        if (pkg == null) return false;
        var setting = snapshot.getPackageStateInternal(name);
        var permission = Objects.requireNonNull(LocalServices.getService(PermissionManagerServiceInternal.class));
        permission.resetRuntimePermissions(pkg, user);
        var binder = Objects.requireNonNull(ServiceManager.checkService("installd"));
        IInstalld installer = IInstalld.Stub.asInterface(binder);
        long inode = setting == null ? 0 : setting.getUserStateOrDefault(user).getCeDataInode();
        installer.clearAppData(pkg.getVolumeUuid(), name, user, 7, inode);
        maintenance.clearAppProfiles(name);
        int appId = android.os.UserHandle.getAppId(pkg.getUid());
        if (appId >= 0) android.security.AndroidKeyStoreMaintenance.clearNamespace(0, android.os.UserHandle.getUid(user, appId));
        var users = Objects.requireNonNull(LocalServices.getService(UserManagerInternal.class));
        var storage = Objects.requireNonNull(LocalServices.getService(android.os.storage.StorageManagerInternal.class));
        int flags = android.os.storage.StorageManager.isCeStorageUnlocked(user) && storage.isCeStoragePrepared(user) ? 3 : users.isUserRunning(user) ? 1 : 0;
        if ((flags & 2) != 0) {
            String abi = setting == null ? com.android.server.pm.parsing.pkg.AndroidPackageUtils.getRawPrimaryCpuAbi(pkg) : setting.getPrimaryCpuAbi();
            String path = pkg.getNativeLibraryDir();
            if (abi != null && !dalvik.system.VMRuntime.is64BitAbi(abi) && new java.io.File(path).exists())
                installer.linkNativeLibraryDirectory(pkg.getVolumeUuid(), name, path, user);
        }
        return true;
    }
    @Override public void checkMemory() {
        enforce(); var owner = LocalServices.getService(com.android.server.storage.DeviceStorageMonitorInternal.class);
        if (owner != null) owner.checkMemory();
    }
    @Override public void sendDeviceCustomizationReady() {
        enforce(); long token = Binder.clearCallingIdentity();
        try { BroadcastHelper.sendDeviceCustomizationReadyBroadcast(); }
        finally { Binder.restoreCallingIdentity(token); }
    }
    @Override public void deletePreloads() {
        enforce(); android.os.FileUtils.deleteContents(android.os.Environment.getDataPreloadsFileCacheDirectory());
    }
}
