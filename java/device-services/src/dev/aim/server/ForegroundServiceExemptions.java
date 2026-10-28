package dev.aim.server;

import static dev.aim.server.IForegroundServiceTypeHost.*;

/**
 * ActiveServices.SystemExemptedFgsTypePermission and AppRestrictionController's
 * three getPotential*ExemptionReason methods, android-16.0.0_r1. AMS's internal
 * reads are supplied by their native owners; the decision order stays in Java.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 */
public final class ForegroundServiceExemptions {
    private ForegroundServiceExemptions() {}

    public interface Source {
        boolean has(int fact);
        boolean exemptedPackage();
        boolean exemptPowerRestrictions();
    }

    public static boolean systemExempted(int uid, Source source) {
        // getPotentialSystemExemptionReason(uid).
        if (uid % 100000 < 10000) return true;
        for (int fact : new int[] {SYSTEM_ALLOWLIST, DEMO, DISALLOW_APPS_CONTROL,
                DEVICE_OWNER, PROFILE_OWNER, PERSISTENT}) {
            if (source.has(fact)) return true;
        }
        // getPotentialSystemExemptionReason(uid, packageName).
        if (source.has(SYSTEM_MODULE) || source.has(CARRIER) || source.exemptedPackage()
                || source.has(PROTECTED) || source.has(ACTIVE_ADMIN)
                || source.exemptPowerRestrictions()) return true;
        // getPotentialUserAllowedExemptionReason: VPN and DIALER are potential
        // exemptions which this custom permission does not accept. A selected
        // reason still prevents falling through to a later, accepted reason.
        if (!source.has(VPN) && !source.has(PLATFORM_VPN) && !source.has(DIALER)) {
            if (source.has(EMERGENCY) || source.has(USER_ALLOWLIST)) return true;
        }
        // ActiveServices only tries KnownPackages when the reason was DENIED.
        // A VPN, dialer or companion reason must not grant via the installer.
        else return false;
        if (source.has(COMPANION)) return false;
        return source.has(INSTALLER);
    }
}
