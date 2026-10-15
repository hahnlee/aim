package dev.aim.server;

import android.app.admin.DevicePolicyManagerInternal;
import android.app.admin.IDevicePolicyManager;
import android.app.role.RoleManager;
import android.content.Context;
import android.content.pm.PackageManagerInternal;
import android.os.Binder;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.os.UserManager;
import android.provider.Settings;
import android.util.SparseArray;
import android.util.EventLog;
import com.android.server.LocalServices;
import com.android.server.storage.DeviceStorageMonitorInternal;
import com.android.server.pm.ProtectedPackages;
import com.android.server.pm.UserManagerInternal;
import java.util.List;

/** Retained original policy objects; no original PMS or Computer forwarding. */
public final class PackagePolicyBridge extends IPackagePolicyBridge.Stub
        implements NativePackageManagerInternal.PolicyOwner,
                NativePackageManagerInternal.PolicyReadOwner {
    private final Context context;
    private final ProtectedPackages protectedPackages;
    private volatile PackageManagerInternal.ExternalSourcesPolicy externalSources;

    public PackagePolicyBridge(Context context) {
        if (context == null) throw new IllegalArgumentException("missing policy context");
        this.context = context;
        protectedPackages = new ProtectedPackages(context);
    }

    public ProtectedPackages protectedPackages() { return protectedPackages; }

    @Override public void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwner,
            SparseArray<String> profileOwners) {
        protectedPackages.setDeviceAndProfileOwnerPackages(deviceOwnerUserId, deviceOwner, profileOwners);
    }
    @Override public void setOwnerProtectedPackages(int userId, List<String> names) {
        protectedPackages.setOwnerProtectedPackages(userId, names);
    }
    @Override public void setExternalSourcesPolicy(PackageManagerInternal.ExternalSourcesPolicy policy) {
        externalSources = policy;
    }
    private static void enforceHost() {
        int uid = Binder.getCallingUid();
        if (uid != Process.SYSTEM_UID && uid != Process.ROOT_UID)
            throw new SecurityException("native package policy caller must be system or root");
    }
    private static UserManagerInternal users() {
        var owner = LocalServices.getService(UserManagerInternal.class);
        if (owner == null) throw new IllegalStateException("user policy owner unavailable");
        return owner;
    }
    @Override public void reportAdminPermissionDenied() {
        enforceHost();
        EventLog.writeEvent(0x534e4554, "128599183", -1, "");
    }
    @Override public boolean isStorageLow() {
        enforceHost();
        long token = Binder.clearCallingIdentity();
        try {
            var monitor = LocalServices.getService(DeviceStorageMonitorInternal.class);
            return monitor != null && monitor.isMemoryLow();
        } finally { Binder.restoreCallingIdentity(token); }
    }
    @Override public int getInstallLocation() {
        enforceHost();
        return Settings.Global.getInt(context.getContentResolver(),
                Settings.Global.DEFAULT_INSTALL_LOCATION, 0);
    }
    @Override public boolean setInstallLocation(int callerPid, int callerUid, int location) {
        enforceHost();
        context.enforcePermission("android.permission.WRITE_SECURE_SETTINGS", callerPid, callerUid,
                "setInstallLocation");
        if (getInstallLocation() == location) return true;
        if (location < 0 || location > 2) return false;
        Settings.Global.putInt(context.getContentResolver(), Settings.Global.DEFAULT_INSTALL_LOCATION,
                location);
        return true;
    }
    @Override public boolean isInstallDisabled(String packageName, int packageUid, int userId) {
        enforceHost();
        return isInstallDisabledForPackage(packageName, packageUid, userId);
    }
    @Override public boolean isInstallDisabledForPackage(String packageName, int packageUid, int userId) {
        var users = users();
        if (users.hasUserRestriction(UserManager.DISALLOW_INSTALL_UNKNOWN_SOURCES, userId)
                || users.hasUserRestriction(UserManager.DISALLOW_INSTALL_UNKNOWN_SOURCES_GLOBALLY,
                        userId)) return true;
        var policy = externalSources;
        return policy != null && policy.getPackageTrustedToInstallApps(packageName, packageUid)
                != com.android.server.pm.NativePackageConstants.USER_TRUSTED;
    }
    @Override public boolean isShellDebuggingRestricted(int userId) {
        enforceHost();
        return users().hasUserRestriction(UserManager.DISALLOW_DEBUGGING_FEATURES, userId);
    }
    @Override public boolean isPackageDeviceAdmin(String packageName, boolean packageExists) {
        enforceHost();
        var dpm = IDevicePolicyManager.Stub.asInterface(ServiceManager.getService("device_policy"));
        var local = LocalServices.getService(DevicePolicyManagerInternal.class);
        if (dpm == null || local == null) return false;
        try {
            var deviceOwner = dpm.getDeviceOwnerComponent(false);
            if (packageName.equals(deviceOwner == null ? null : deviceOwner.getPackageName())) return true;
            int[] userIds = users().getUserIds();
            for (int userId : userIds) if (dpm.packageHasActiveAdmins(packageName, userId)) return true;
            if (!packageExists) return false;
            var roles = context.getSystemService(RoleManager.class);
            if (roles == null) throw new IllegalStateException("device management role owner unavailable");
            long token = Binder.clearCallingIdentity();
            try {
                for (int userId : userIds) {
                    List<String> holders = roles.getRoleHoldersAsUser(RoleManager.ROLE_DEVICE_POLICY_MANAGEMENT,
                            UserHandle.of(userId));
                    if (!holders.isEmpty() && packageName.equals(holders.get(0))
                            && local.isUserOrganizationManaged(userId)) return true;
                }
            } finally { Binder.restoreCallingIdentity(token); }
            return false;
        } catch (RemoteException failure) {
            // Original PMS returns false for a remote DPM failure.
            return false;
        }
    }
    @Override public boolean queryPackageStateProtected(String packageName, int userId) {
        enforceHost();
        return isPackageStateProtected(packageName, userId);
    }
    @Override public boolean queryPackageDataProtected(String packageName, int userId) {
        enforceHost();
        return isPackageDataProtected(userId, packageName);
    }
    @Override public boolean isPackageStateProtected(String packageName, int userId) {
        return protectedPackages.isPackageStateProtected(userId, packageName);
    }
    @Override public boolean isPackageDataProtected(int userId, String packageName) {
        return protectedPackages.isPackageDataProtected(userId, packageName);
    }
}
