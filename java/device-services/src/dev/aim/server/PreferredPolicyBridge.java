package dev.aim.server;

import android.app.ActivityThread;
import android.app.role.RoleManager;
import android.content.Context;
import android.content.Intent;
import android.os.Binder;
import android.os.UserHandle;
import com.android.server.LocalServices;
import com.android.server.FgThread;
import com.android.server.pm.UserManagerService;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.net.NetworkPolicyManagerInternal;

/** Independent original UM, Role, permission and network owners; never PMS. */
final class PreferredPolicyBridge {
    private static Context context() {
        var thread = ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context owner unavailable");
        return thread.getSystemContext();
    }
    static int crossProfileAccess(int source, int target) {
        var users = UserManagerService.getInstance();
        if (users == null) throw new IllegalStateException("UserManager owner unavailable");
        // Authorization uses the actual original caller UID in the native owner,
        // not this privileged bridge's Binder UID.
        return users.getCrossProfileIntentFilterAccessControl(source, target);
    }
    static String roleHolder(String role, int user) {
        var manager = context().getSystemService(RoleManager.class);
        if (manager == null) return null; // Original DefaultAppProvider's pre-role-service path.
        long token = Binder.clearCallingIdentity();
        try {
            var holders = manager.getRoleHoldersAsUser(role, UserHandle.of(user));
            return holders.isEmpty() ? null : holders.get(0);
        } finally { Binder.restoreCallingIdentity(token); }
    }
    static void roleHolder(String role, String packageName, int user, boolean broadcast) {
        var manager = context().getSystemService(RoleManager.class);
        if (manager == null) throw new IllegalStateException("RoleManager write owner unavailable");
        long token = Binder.clearCallingIdentity();
        try {
            manager.addRoleHolderAsUser(role, packageName, 0, UserHandle.of(user), FgThread.getExecutor(), successful -> {
                if (successful) { if (broadcast) preferredChanged(user); }
                else android.util.Slog.e("PackageManager", "Failed to set role " + role + " to " + packageName);
            });
        } finally { Binder.restoreCallingIdentity(token); }
    }
    static void preferredChanged(int user) {
        FgThread.getExecutor().execute(() -> {
            if (android.app.ActivityManager.getService() == null) return;
            Intent intent = new Intent("android.intent.action.ACTION_PREFERRED_ACTIVITY_CHANGED");
            intent.putExtra("android.intent.extra.user_handle", user);
            intent.addFlags(0x04000000); // FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT.
            context().sendBroadcastAsUser(intent, UserHandle.of(user));
        });
    }
    static void resetPermissions(int user) {
        var permissions = LocalServices.getService(PermissionManagerServiceInternal.class);
        if (permissions == null) throw new IllegalStateException("permission reset owner unavailable");
        permissions.resetRuntimePermissionsForUser(user);
    }
    static void resetNetwork(int user) {
        var network = LocalServices.getService(NetworkPolicyManagerInternal.class);
        if (network == null) throw new IllegalStateException("network policy reset owner unavailable");
        network.resetUserState(user);
    }
}
