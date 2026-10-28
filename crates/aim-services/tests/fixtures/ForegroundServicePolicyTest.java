import dev.aim.server.ForegroundServiceExemptions;
import static dev.aim.server.IForegroundServiceTypeHost.*;
import java.util.Set;

/** ActiveServices.SystemExemptedFgsTypePermission, android-16.0.0_r1. */
public final class ForegroundServicePolicyTest {
    static ForegroundServiceExemptions.Source source(Set<Integer> facts) {
        return new ForegroundServiceExemptions.Source() {
            public boolean has(int fact) { return facts.contains(fact); }
            public boolean exemptedPackage() { return false; }
            public boolean exemptPowerRestrictions() { return false; }
        };
    }
    public static void main(String[] args) {
        // The custom permission is reached after the original policy has checked
        // FOREGROUND_SERVICE_SYSTEM_EXEMPTED and found no exact-alarm/VPN grant.
        for (int fact : new int[] {SYSTEM_ALLOWLIST, DEMO, DISALLOW_APPS_CONTROL,
                DEVICE_OWNER, PROFILE_OWNER, PERSISTENT, SYSTEM_MODULE, CARRIER,
                PROTECTED, ACTIVE_ADMIN, EMERGENCY, USER_ALLOWLIST, INSTALLER}) {
            if (!ForegroundServiceExemptions.systemExempted(10123, source(Set.of(fact))))
                throw new AssertionError("denied exemption " + fact);
        }
        if (ForegroundServiceExemptions.systemExempted(10123, source(Set.of())))
            throw new AssertionError("granted without an exemption");
        // Potential exemptions that ActiveServices does not accept must prevent
        // falling through to a later accepted reason, even KnownPackages.
        for (int fact : new int[] {VPN, PLATFORM_VPN, DIALER, COMPANION}) {
            if (ForegroundServiceExemptions.systemExempted(10123,
                    source(Set.of(fact, INSTALLER)))) throw new AssertionError("reason order " + fact);
        }
        var unavailable = new ForegroundServiceExemptions.Source() {
            public boolean has(int fact) { throw new UnsupportedOperationException("owner #1552"); }
            public boolean exemptedPackage() { return false; }
            public boolean exemptPowerRestrictions() { return false; }
        };
        try {
            ForegroundServiceExemptions.systemExempted(10123, unavailable);
            throw new AssertionError("swallowed missing owner");
        } catch (UnsupportedOperationException expected) {
            if (!expected.getMessage().contains("#1552")) throw expected;
        }
        if (!ForegroundServiceExemptions.systemExempted(1000, unavailable))
            throw new AssertionError("system UID");
    }
}
