package dev.aim.server;

import com.android.server.devicepolicy.DevicePolicyCacheImpl;

/**
 * Non-hot policy queries retain android-16.0.0_r1's original default cache.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 */
final class DevicePolicyDefaults extends IDevicePolicyDefaults.Stub {
    private final android.content.Context context;
    DevicePolicyDefaults(android.content.Context context) { this.context = context; }
    private final DevicePolicyCacheImpl defaults = new DevicePolicyCacheImpl();

    @Override public int getContentProtectionPolicyDefault(int userId) {
        Bridge.enforceSystemUid();
        return defaults.getContentProtectionPolicy(userId);
    }
    // DPMS android-16.0.0_r1:22739-22745, 18773-18828. Read the live
    // native users/accounts instead of maintaining DPMS's asynchronous cache.
    @Override public boolean shouldAllowRoleQualification() {
        Bridge.enforceSystemUid();
        var users = com.android.server.LocalServices.getService(
                com.android.server.pm.UserManagerInternal.class).getUsers(true);
        long allowed = android.os.UserManager.isHeadlessSystemUserMode() ? 2 : 1;
        if (users.stream().filter(u -> !u.isForTesting()).count() > allowed) return false;
        for (var user : users) {
            var accounts = context.createContextAsUser(android.os.UserHandle.of(user.id), 0)
                    .getSystemService(android.accounts.AccountManager.class);
            for (var account : accounts.getAccounts()) {
                if (hasFeature(accounts, account, "android.account.DEVICE_OR_PROFILE_OWNER_DISALLOWED")
                        || !hasFeature(accounts, account, "android.account.DEVICE_OR_PROFILE_OWNER_ALLOWED")) return false;
            }
        }
        return true;
    }
    private static boolean hasFeature(android.accounts.AccountManager manager,
            android.accounts.Account account, String feature) {
        try { return manager.hasFeatures(account, new String[] {feature}, null, null)
                .getResult(30, java.util.concurrent.TimeUnit.SECONDS); }
        catch (Exception e) { android.util.Slog.w("DevicePolicyManager", "Failed to get account feature", e); return false; }
    }
    @Override public boolean isUsbDataSignalingEnabled(byte[] policies) {
        Bridge.enforceSystemUid();
        return com.android.server.devicepolicy.UsbDataSignalingPolicy.isEnabled(policies);
    }
}
