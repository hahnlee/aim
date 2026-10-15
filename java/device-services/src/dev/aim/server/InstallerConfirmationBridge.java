package dev.aim.server;

import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.os.Binder;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.SystemClock;
import android.os.UserHandle;
import android.provider.DeviceConfig;
import android.app.admin.DevicePolicyManagerInternal;
import com.android.internal.compat.IPlatformCompat;
import com.android.server.LocalServices;
import java.util.Objects;

/** Original PIS leaves without a PackageInstallerSession or PMS follower. */
public final class InstallerConfirmationBridge extends IInstallerConfirmationBridge.Stub {
    private final PackagePolicyBridge packagePolicy;
    private final boolean dependencyInstallerEnabled;
    public InstallerConfirmationBridge(PackagePolicyBridge packagePolicy,
            boolean dependencyInstallerEnabled) {
        this.packagePolicy = Objects.requireNonNull(packagePolicy);
        // Exact image flag input, supplied by the native bootstrap's pinned
        // policy extraction; never an environment switch or a default guess.
        this.dependencyInstallerEnabled = dependencyInstallerEnabled;
    }
    private static void enforceNative() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) throw new SecurityException("installer confirmation requires native system UID");
    }
    @Override public boolean isDeviceOwnerOrAffiliated(String installer, int uid, int user) {
        enforceNative();
        if (user != UserHandle.getUserId(uid)) return false;
        DevicePolicyManagerInternal policy = LocalServices.getService(DevicePolicyManagerInternal.class);
        // This is the original PIS early-bootstrap null-service branch.
        return policy != null && policy.canSilentlyInstallPackage(installer, uid);
    }
    @Override public boolean isSilentTargetAllowed(String name, int targetSdk) {
        enforceNative();
        if (targetSdk == Integer.MAX_VALUE) return false;
        ApplicationInfo app = new ApplicationInfo();
        app.packageName = name;
        app.targetSdkVersion = targetSdk;
        IPlatformCompat compat = IPlatformCompat.Stub.asInterface(ServiceManager.getService("platform_compat"));
        if (compat == null) throw new IllegalStateException("installer PlatformCompat unavailable");
        try { return compat.isChangeEnabled(325888262L, app); }
        catch (RemoteException failure) {
            // Original isTargetSdkConditionSatisfied logs and declines silent
            // installation when this remote policy request fails.
            android.util.Slog.e("PackageInstallerSession", "Failed to get a response from PLATFORM_COMPAT_SERVICE", failure);
            return false;
        }
    }
    @Override public boolean isInstallDisabled(String name, int uid, int user) {
        enforceNative(); return packagePolicy.isInstallDisabledForPackage(name, uid, user);
    }
    @Override public boolean isDependencyInstallerEnabled() { enforceNative(); return dependencyInstallerEnabled; }
    @Override public boolean isUpdateOwnershipEnabled() {
        enforceNative();
        long identity = Binder.clearCallingIdentity();
        try { return DeviceConfig.getBoolean("package_manager_service", "is_update_ownership_enforcement_available", true); }
        finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public long getUptimeMillis() { enforceNative(); return SystemClock.uptimeMillis(); }
}
