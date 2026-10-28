package dev.aim.server;

import android.os.Binder;
import com.android.server.devicepolicy.DevicePolicyCacheImpl;

public final class DevicePolicyDefaultsTest {
    public static void main(String[] args) {
        DevicePolicyDefaults bridge = new DevicePolicyDefaults();
        for (int user : new int[] {-1, 0, 10}) {
            if (bridge.getContentProtectionPolicyDefault(user) != 37
                    || DevicePolicyCacheImpl.user != user)
                throw new AssertionError("original cache result/argument changed");
        }
        for (int uid : new int[] {0, 2000, 10100, 101000}) {
            Binder.uid = uid;
            int cacheCalls = DevicePolicyCacheImpl.calls;
            try {
                bridge.getContentProtectionPolicyDefault(0);
                throw new AssertionError("non-system caller accepted");
            } catch (SecurityException expected) {}
            if (cacheCalls != DevicePolicyCacheImpl.calls)
                throw new AssertionError("helper ran before authentication");
        }
        Binder.uid = 1000;
        RuntimeException failure = new IllegalStateException("original failure");
        DevicePolicyCacheImpl.failure = failure;
        try { bridge.getContentProtectionPolicyDefault(0); throw new AssertionError("failure lost"); }
        catch (RuntimeException expected) {
            if (expected != failure) throw new AssertionError(expected);
        }
    }
}
