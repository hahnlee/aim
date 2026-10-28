package com.android.server;
/** Lifecycle boundary double; this does not implement or test original service bodies. */
public abstract class SystemService {
    public static final int PHASE_LOCK_SETTINGS_READY=480, PHASE_SYSTEM_SERVICES_READY=500,
            PHASE_DEVICE_SPECIFIC_SERVICES_READY=520, PHASE_ACTIVITY_MANAGER_READY=550,
            PHASE_THIRD_PARTY_APPS_CAN_START=600, PHASE_BOOT_COMPLETED=1000;
    public static final class TargetUser {
        public TargetUser(android.content.pm.UserInfo info) {}
    }
    public SystemService(android.content.Context context) {
        if (context != android.app.ActivityThread.context) throw new AssertionError("lost context");
        event("construct");
    }
    private void event(String action) {
        dev.aim.server.SystemProcessHostTest.events.add(getClass().getSimpleName()+":"+action);
    }
    public void onStart() { event("start"); }
    public void onBootPhase(int phase) { event("phase="+phase); }
    public void onUserStarting(TargetUser user) { event("starting"); }
    public void onUserSwitching(TargetUser from, TargetUser to) {
        if (from != null || to == null) throw new AssertionError("initial user switch");
        event("switching");
    }
    public void onUserUnlocking(TargetUser user) { event("unlocking"); }
    public void onUserUnlocked(TargetUser user) { event("unlocked"); }
}
