// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

import android.content.Context;

public abstract class SystemService {
    public static final int PHASE_DEVICE_SPECIFIC_SERVICES_READY = 520;
    public static final int PHASE_THIRD_PARTY_APPS_CAN_START = 600;

    public static final class TargetUser {
        public TargetUser(android.content.pm.UserInfo userInfo) { throw new RuntimeException("stub"); }
        public int getUserIdentifier() { throw new RuntimeException("stub"); }
    }

    public SystemService(Context context) { throw new RuntimeException("stub"); }
    public abstract void onStart();
    public void onBootPhase(int phase) { throw new RuntimeException("stub"); }
    public void onUserStarting(TargetUser user) { throw new RuntimeException("stub"); }
    public void onUserUnlocking(TargetUser user) { throw new RuntimeException("stub"); }
    public void onUserSwitching(TargetUser from, TargetUser to) { throw new RuntimeException("stub"); }
    public void onUserStopped(TargetUser user) { throw new RuntimeException("stub"); }
    public final Context getContext() { throw new RuntimeException("stub"); }
}
