package dev.aim.server;

import android.os.Binder;
import com.android.role.controller.model.Role;
import com.android.role.controller.model.Roles;
import com.android.role.controller.util.RoleFlags;
import com.android.server.devicepolicy.PolicyState;

public final class StartTimeReadsTest {
    private static void fails(Class<? extends RuntimeException> type, Runnable action) {
        try { action.run(); throw new AssertionError("accepted"); }
        catch (RuntimeException error) { if (!type.isInstance(error)) throw new AssertionError(error); }
    }
    public static void main(String[] args) {
        var role = new Role(); role.exclusivity = 2; Roles.roles.put("exclusive", role);
        var roles = new RoleQueries(null);
        var policy = new DevicePolicyDefaults();
        for (int uid : new int[]{0, 2000, 10100, 101000}) {
            Binder.uid = uid;
            int calls = Roles.calls;
            fails(SecurityException.class, () -> roles.enforceProfileGroupExclusiveRole("exclusive"));
            fails(SecurityException.class, () -> policy.isUsbDataSignalingEnabled(new byte[0]));
            if (Roles.calls != calls) throw new AssertionError("role definitions read before authentication");
        }
        Binder.uid = 1000;
        RoleFlags.available = false;
        fails(IllegalStateException.class, () -> roles.enforceProfileGroupExclusiveRole(null));
        RoleFlags.available = true;
        for (String name : new String[]{null, "", "missing"})
            fails(IllegalArgumentException.class, () -> roles.enforceProfileGroupExclusiveRole(name));
        roles.enforceProfileGroupExclusiveRole("exclusive");
        role.exclusivity = 1;
        fails(IllegalArgumentException.class, () -> roles.enforceProfileGroupExclusiveRole("exclusive"));

        if (!policy.isUsbDataSignalingEnabled(new byte[0])) throw new AssertionError("absent engine state");
        for (boolean admins : new boolean[]{false, true}) for (Boolean resolved : new Boolean[]{null, false, true}) {
            PolicyState.admins = admins; PolicyState.resolved = resolved;
            for (String scope : new String[]{"local", "global"}) {
                String xml = "<" + scope + "-policy-entry><policy-key-entry policy-identifier='usbDataSignaling'/>"
                        + "<policy-state-entry/></" + scope + "-policy-entry>";
                int reads = PolicyState.reads;
                boolean actual = policy.isUsbDataSignalingEnabled(xml.getBytes(java.nio.charset.StandardCharsets.UTF_8));
                boolean expected = !scope.equals("global") || !admins || resolved == null || resolved;
                if (actual != expected || PolicyState.reads - reads != (scope.equals("global") ? 1 : 0))
                    throw new AssertionError("resolved global-only policy changed");
            }
        }
    }
}
