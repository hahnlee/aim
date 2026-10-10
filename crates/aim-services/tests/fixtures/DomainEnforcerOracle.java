/** Executes the pinned Enforcer with controlled permission/identity/visibility owners. */
public final class DomainEnforcerOracle {
    private static final String[] PERMISSIONS = {"DUMP", "QUERY_ALL_PACKAGES", "DOMAIN_VERIFICATION_AGENT", "INTENT_FILTER_VERIFICATION_AGENT", "INTERACT_ACROSS_USERS", "UPDATE_DOMAIN_VERIFICATION_USER_SELECTION", "INTERACT_ACROSS_USERS_FULL", "SET_PREFERRED_APPLICATIONS"};
    private static final class Permissions extends android.content.ContextWrapper {
        int mask;
        Permissions() { super(null); }
        @Override public int checkPermission(String name, int pid, int uid) {
            for (int i = 0; i < PERMISSIONS.length; i++) if (name.equals("android.permission." + PERMISSIONS[i])) return (mask & (1 << i)) != 0 ? 0 : -1;
            throw new AssertionError("unexpected domain permission: " + name);
        }
        @Override public void enforcePermission(String name, int pid, int uid, String message) { if (checkPermission(name, pid, uid) != 0) throw new SecurityException(message); }
    }
    private static final class Identity implements com.android.server.pm.verify.domain.proxy.DomainVerificationProxy, com.android.server.pm.verify.domain.DomainVerificationEnforcer.Callback {
        int missing; boolean hidden, verifier;
        public android.content.ComponentName getComponentName() { throw new AssertionError("unused verifier component owner"); }
        public boolean isCallerVerifier(int uid) { return verifier; }
        public boolean doesUserExist(int user) { return (user == 0 || user == 10) && user != missing; }
        public boolean filterAppAccess(String name, int uid, int user) { if (name != null && !"fixture.domains".equals(name)) throw new AssertionError("foreign domain visibility package"); return hidden; }
        public void sendBroadcastForPackages(java.util.Set<String> names) { throw new AssertionError("authorization broadcast"); }
        public boolean runMessage(int code, Object object) { throw new AssertionError("authorization message"); }
    }
    static void verify(java.io.File directory) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-enforcer.input").toPath());
        var in = android.os.Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var context = new Permissions(); var identity = new Identity();
            var enforcer = new com.android.server.pm.verify.domain.DomainVerificationEnforcer(context); enforcer.setCallback(identity);
            int count = in.readInt();
            for (int i = 0; i < count; i++) {
                int uid = in.readInt(); identity.verifier = in.readInt() != 0; context.mask = in.readInt();
                int target = in.readInt(); identity.missing = in.readInt(); identity.hidden = in.readInt() != 0;
                int kind = in.readInt(), expected = in.readInt(), actual = 1;
                try {
                    switch (kind) {
                        case 0 -> enforcer.assertInternal(uid);
                        case 1 -> enforcer.assertApprovedQuerent(uid, identity);
                        case 2 -> enforcer.assertApprovedVerifier(uid, identity);
                        case 3 -> actual = enforcer.assertApprovedUserStateQuerent(uid, 0, "fixture.domains", target) ? 1 : 0;
                        case 4 -> actual = enforcer.assertApprovedUserSelector(uid, 0, "fixture.domains", target) ? 1 : 0;
                        case 5 -> enforcer.assertOwnerQuerent(uid, 0, target);
                        case 6 -> actual = enforcer.callerIsLegacyUserSelector(uid, 0, "fixture.domains", target) ? 1 : 0;
                        case 7 -> actual = enforcer.callerIsLegacyUserQuerent(uid, 0, "fixture.domains", target) ? 1 : 0;
                        case 8 -> actual = enforcer.assertApprovedUserSelector(uid, 0, null, target) ? 1 : 0;
                        case 9 -> actual = enforcer.assertApprovedUserStateQuerent(uid, 0, null, target) ? 1 : 0;
                        default -> throw new AssertionError("unknown authorization case");
                    }
                } catch (SecurityException denied) { actual = -1; }
                if (actual != expected) throw new AssertionError("domain authorization differs case=" + i + " operation=" + kind + " uid=" + uid + " permissions=" + context.mask + " actual=" + actual + " expected=" + expected);
            }
            if (in.dataAvail() != 0) throw new AssertionError("trailing domain authorization expectations");
        } finally { in.recycle(); }
    }
}
