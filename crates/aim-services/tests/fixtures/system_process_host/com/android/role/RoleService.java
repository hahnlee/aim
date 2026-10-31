package com.android.role;
/** Lifecycle boundary double; no role selection or grants are simulated. */
public final class RoleService extends com.android.server.SystemService {
    public RoleService(android.content.Context context) {
        super(context);
        if (com.android.server.LocalManagerRegistry.getManager(
                com.android.server.role.RoleServicePlatformHelper.class) == null)
            throw new AssertionError("missing role platform helper");
    }
    @Override public void onStart() {
        super.onStart();
        android.os.ServiceManager.addService("role", new android.os.Binder());
    }
}
