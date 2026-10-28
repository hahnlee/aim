package dev.aim.server;

import com.android.server.devicepolicy.DevicePolicyCacheImpl;

/**
 * Non-hot policy queries retain android-16.0.0_r1's original default cache.
 * No policy producer is attached: the native owner admits only owner-less state.
 */
final class DevicePolicyDefaults extends IDevicePolicyDefaults.Stub {
    private final DevicePolicyCacheImpl defaults = new DevicePolicyCacheImpl();

    @Override public int getContentProtectionPolicyDefault(int userId) {
        Bridge.enforceSystemUid();
        return defaults.getContentProtectionPolicy(userId);
    }
}
