package dev.aim.server;

import android.content.pm.UserInfo;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import com.android.server.pm.permission.LegacyPermissionState;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import java.util.*;

/** Explicit owners exercise callback order on original ART; no live permission service claim. */
public final class PackagePermissionLifecycleOracle {
    static final class Owners implements PermissionManagerServiceInternal,
            LegacyPermissionManagerInternal, PackagePermissionLifecycle.StateOwner {
        final List<String> events = new ArrayList<>();
        final Map<Integer, String> fingerprints = new HashMap<>();
        final Object packageLock = new Object();
        boolean failGrant;
        public Set<String> getInstalledPermissions(String name) { throw new AssertionError(); }
        public Set<String> getGrantedPermissions(String name, int user) { throw new AssertionError(); }
        public int[] getGidsForUid(int uid) { throw new AssertionError(); }
        public LegacyPermissionState getLegacyPermissionState(int app) { throw new AssertionError(); }
        public String getDefaultPermissionGrantFingerprint(int user) { events.add("get:" + user); return fingerprints.get(user); }
        public void setDefaultPermissionGrantFingerprint(String value, int user) { events.add("set:" + user); fingerprints.put(user, value); }
        public void onSystemReady() { events.add("ready"); }
        public void onUserCreated(int user) { events.add("created:" + user); }
        public void onUserRemoved(int user) { events.add("removed:" + user); }
        public void writeLegacyPermissionStateTEMP() { checkLock(); events.add("write"); }
        public void readLegacyPermissionStateTEMP() { checkLock(); events.add("read"); }
        public void setLocationPackagesProvider(PackagesProvider provider) { throw new AssertionError(); }
        public void setLocationExtraPackagesProvider(PackagesProvider provider) { throw new AssertionError(); }
        public void grantDefaultPermissions(int user) { events.add("grant:" + user); if (failGrant) throw new IllegalStateException("grant failed"); }
        public void scheduleReadDefaultPermissionExceptions() { events.add("schedule"); }
        public Object packageLock() { return packageLock; }
        void checkLock() { if (!Thread.holdsLock(packageLock)) throw new AssertionError("missing package lock"); }
        public void readPermissionStateForUser(int user) { checkLock(); events.add("state:" + user); }
        public void clearDomainUser(int user) { events.add("domain:" + user); }
    }
    static UserInfo user(int id) { UserInfo value = new UserInfo(); value.id = id; return value; }
    static void expect(Owners o, String... events) {
        if (!o.events.equals(Arrays.asList(events))) throw new AssertionError(o.events);
        o.events.clear();
    }
    public static void main(String[] args) {
        Owners o = new Owners();
        PackagePermissionLifecycle life = new PackagePermissionLifecycle(o, o, o, "current");
        o.fingerprints.put(1, "current");
        life.systemReady(Arrays.asList(user(0), user(1), user(2)));
        expect(o, "ready", "get:0", "get:1", "get:2", "grant:0", "set:0", "grant:2", "set:2");
        life.systemReady(Arrays.asList(user(0), user(1)));
        expect(o, "ready", "get:0", "get:1", "schedule");
        life.systemReady(Collections.emptyList());
        expect(o, "ready", "schedule");
        life.onNewUserCreated(3, false);
        expect(o, "created:3", "grant:3", "set:3", "domain:3");
        // Original's precreated read returns 'upgrade needed', so an outdated
        // fingerprint suppresses this callback cohort; current triggers it.
        life.onNewUserCreated(4, true);
        expect(o, "write", "state:4", "read", "get:4");
        life.onNewUserCreated(3, true);
        expect(o, "write", "state:3", "read", "get:3", "created:3", "grant:3", "set:3", "domain:3");
        life.onUserRemoved(3);
        expect(o, "removed:3");
        o.failGrant = true;
        try { life.onNewUserCreated(5, false); throw new AssertionError("swallowed failure"); }
        catch (IllegalStateException expected) { if (!expected.getMessage().equals("grant failed")) throw expected; }
        expect(o, "created:5", "grant:5");
        if (o.fingerprints.containsKey(5)) throw new AssertionError("fingerprint on failed grant");
        System.out.println("permission lifecycle ordering checks passed (explicit owners on original ART)");
    }
}
