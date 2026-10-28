package dev.aim.server;

import static dev.aim.server.PermissionBootPolicy.*;

/** Android 16 r1 AppIdPermissionPolicy.kt:1066-1070,1183-1227,1538-1548. */
public final class PermissionBootPolicyTest {
    private static final PermissionBootPolicy POLICY = new PermissionBootPolicy();
    private static final int USER_SET = 1 << 5;
    private static void check(String name, int level, int info, int target,
            int old, int computed, int expected) {
        int actual = POLICY.evaluatePermission(name, level, info, target, old, computed);
        if (actual != expected) throw new AssertionError(name + ": " + actual + " != " + expected);
    }
    public static void main(String[] args) {
        int granted = RUNTIME_GRANTED | USER_SET;
        for (int info : new int[] {4, 8}) {
            int restriction = info == 4 ? RESTRICTION_REVOKED : SOFT_RESTRICTED;
            for (int grant : new int[] {0, granted}) {
                int denied = grant | restriction;
                check("restricted", 1, info, 36, denied, denied, denied);
                for (int source : new int[] {INSTALLER_EXEMPT, SYSTEM_EXEMPT, UPGRADE_EXEMPT,
                        MASK_EXEMPT}) {
                    int exempt = denied | source;
                    check("restricted", 1, info, 36, exempt, exempt, grant | source);
                }
                // Previously unevaluated permissions receive the original upgrade exemption.
                check("new", 1, info, 36, grant, grant, grant | UPGRADE_EXEMPT);
            }
        }
        for (String storage : new String[] {"android.permission.READ_EXTERNAL_STORAGE",
                "android.permission.WRITE_EXTERNAL_STORAGE"}) {
            int old = granted | SOFT_RESTRICTED;
            check(storage, 1, 8, 28, old, old, old);
            check(storage, 1, 8, 29, old, old, granted);
            // Shared UID input is the minimum SDK of its requesting packages.
            check(storage, 1, 8, Math.min(28, 36), old, old, old);
        }
        check("no longer restricted", 1, 0, 36, granted | MASK_RESTRICTED,
                granted | MASK_RESTRICTED, granted);
        for (int base : new int[] {2, 4}) {
            for (int old : new int[] {0, ROLE, RUNTIME_GRANTED, ROLE | RUNTIME_GRANTED}) {
                check("role", base | 0x4000000, 0, 36, old, 0, old);
                check("role with static grant", base | 0x4000000, 0, 36, old, 4, old | 4);
            }
        }
    }
}
