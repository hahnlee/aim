package com.android.server.accessibility;
public final class AccessibilityManagerService$Lifecycle extends com.android.server.SystemService {
    public AccessibilityManagerService$Lifecycle(android.content.Context c) { super(c); }
    @Override public void onBootPhase(int phase) {
        if (phase == 600) android.os.ServiceManager.requireSafetyCenter();
        super.onBootPhase(phase);
    }
}
