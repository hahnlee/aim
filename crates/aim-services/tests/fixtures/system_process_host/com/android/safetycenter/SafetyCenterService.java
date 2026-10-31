package com.android.safetycenter;
/** Lifecycle/publication boundary only; original semantics are checked separately. */
public final class SafetyCenterService extends com.android.server.SystemService {
    public SafetyCenterService(android.content.Context context) { super(context); }
    @Override public void onStart() {
        super.onStart();
        android.os.ServiceManager.addService("safety_center", new android.os.Binder());
    }
}
