/* Copyright (C) 2026 The Android Open Source Project
 * SPDX-License-Identifier: Apache-2.0
 */
package dev.aim.server;

/**
 * android-16.0.0_r1 frameworks/base/services/permission/java/com/android/server/
 * permission/access/permission/AppIdPermissionPolicy.kt:1066-1070,1183-1227,1538-1548.
 * Its evaluator is private and requires the Java AccessState graph. Extract only
 * these cold rules over a typed projection; the native owner retains persistence.
 * PMSImpl's equivalent is services/core/java/com/android/server/pm/permission/
 * PermissionManagerServiceImpl.java:2770-2816,2818-2934; updateAllPermissions and
 * updatePermissions:3932-3951,4027-4050. Modern access flags also retain grants
 * independently of restrictions (PermissionFlags.kt:330-346).
 */
final class PermissionBootPolicy extends IPermissionBootPolicy.Stub {
    // PermissionFlags.kt:84,90,211-249,325-328 (access.abx representation).
    static final int ROLE = 1 << 3, RUNTIME_GRANTED = 1 << 4;
    static final int INSTALLER_EXEMPT = 1 << 15, SYSTEM_EXEMPT = 1 << 16;
    static final int UPGRADE_EXEMPT = 1 << 17, RESTRICTION_REVOKED = 1 << 18;
    static final int SOFT_RESTRICTED = 1 << 19;
    static final int MASK_EXEMPT = INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT;
    static final int MASK_RESTRICTED = RESTRICTION_REVOKED | SOFT_RESTRICTED;
    @Override public int evaluatePermission(String name, int protectionLevel, int infoFlags,
            int targetSdkVersion, int oldFlags, int newFlags) {
        Bridge.enforceSystemUid();
        int base = protectionLevel & 0xf;
        if (base == 2 || base == 4) {
            // PMS preserves role grants here; RoleManager/PermissionController
            // decide new grants later. Never infer them from role holder lists.
            if ((protectionLevel & 0x4000000) != 0) {
                newFlags |= oldFlags & (ROLE | RUNTIME_GRANTED);
            }
        } else if (base == 1) {
            boolean hardRestricted = (infoFlags & 4) != 0;
            boolean softRestricted = (infoFlags & 8) != 0;
            // All three allowlist sources are persisted exemption bits:
            // PMSImpl.java:1140-1176 (getAllowlistedRestrictedPermissionsInternal).
            boolean exempt = (newFlags & MASK_EXEMPT) != 0;
            if ((hardRestricted || softRestricted) && !exempt
                    && (newFlags & MASK_RESTRICTED) == 0) {
                newFlags |= UPGRADE_EXEMPT;
                exempt = true;
            }
            newFlags = hardRestricted && !exempt
                    ? newFlags | RESTRICTION_REVOKED : newFlags & ~RESTRICTION_REVOKED;
            boolean storageExempt = targetSdkVersion >= 29
                    && ("android.permission.READ_EXTERNAL_STORAGE".equals(name)
                            || "android.permission.WRITE_EXTERNAL_STORAGE".equals(name));
            newFlags = softRestricted && !exempt && !storageExempt
                    ? newFlags | SOFT_RESTRICTED : newFlags & ~SOFT_RESTRICTED;
        }
        return newFlags;
    }
}
