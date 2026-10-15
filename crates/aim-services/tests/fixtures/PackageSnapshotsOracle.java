package com.android.server.pm;

import android.os.UserHandle;
import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.PackageStateInternal;
import dev.aim.server.PackageSnapshots;
import java.io.File;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/** Original interface/record objects; controlled native visibility responses. */
public final class PackageSnapshotsOracle {
    private static PackageStateInternal state(String name, int appId) {
        var setting = new PackageSetting(name, null, new File("/data/app/" + name),
                0, 0, new UUID(1, appId)).setAppId(appId);
        return (PackageStateInternal)(Object)new PackageSetting(setting, true);
    }
    private static void check(boolean condition) {
        if (!condition) throw new AssertionError("snapshot contract changed");
    }
    private static void closed(Runnable get) {
        try { get.run(); throw new AssertionError("closed scope was readable"); }
        catch (IllegalStateException expected) {
            check("Snapshot already closed".equals(expected.getMessage()));
        }
    }
    private static void immutable(Runnable write) {
        try { write.run(); throw new AssertionError("snapshot map was mutable"); }
        catch (UnsupportedOperationException expected) {}
    }
    public static void main(String[] args) {
        PackageStateInternal a = state("a", 19001), b = state("b", 19002);
        Map<String, PackageState> input = new LinkedHashMap<>();
        input.put("a", a); input.put("b", b);
        List<String> calls = new java.util.ArrayList<>();
        var uncommitted = state("b", 29002);
        var graphs = Map.of(42L, Map.of("a", a, "b", b), 43L, Map.of("b", uncommitted));
        List<String> lookups = new java.util.ArrayList<>();
        PackageSnapshots.Owner visibility = new PackageSnapshots.Owner() {
            public String getFilteredPackageName(long version, String name, int uid, int user) {
                lookups.add(version + ":" + name + ":" + uid + ":" + user);
                // Controlled native response includes internal-name resolution.
                var names = Map.of("a", "a", "b", "b", "alias", "a");
                String resolved = names.get(name);
                if (resolved == null) return null;
                PackageState pkg = graphs.get(version).get(resolved);
                return pkg == null || shouldFilter(version, pkg, uid, user) ? null : resolved;
            }
            public boolean shouldFilter(long version, PackageState pkg, int uid, int user) {
                calls.add(version + ":" + pkg.getPackageName() + ":" + pkg.getAppId()
                        + ":" + uid + ":" + user);
                return pkg == b;
            }
        };
        var data = new PackageSnapshots.Data(42, input, Map.of("b", b), Map.of(), visibility);
        input.clear();
        var parent = PackageSnapshots.unfiltered(data);
        check(parent.getPackageStates().size() == 2);
        check(parent.getDisabledSystemPackageStates().get("b") == b);
        check(parent.getSharedUsers().isEmpty());
        immutable(() -> parent.getPackageStates().clear());
        immutable(() -> parent.getDisabledSystemPackageStates().clear());
        immutable(() -> parent.getSharedUsers().clear());
        var child = parent.filtered(1019000, UserHandle.of(10));
        check(child.getPackageState("a") == a);
        check(child.getPackageState("b") == null);
        check(child.getPackageState("missing") == null);
        check(calls.equals(List.of("42:a:19001:1019000:10", "42:b:19002:1019000:10")));
        check(child.getPackageState("alias") == a);
        check(lookups.equals(List.of("42:a:1019000:10", "42:b:1019000:10",
                "42:missing:1019000:10", "42:alias:1019000:10")));
        var filtered = child.getPackageStates();
        check(filtered.equals(Map.of("a", a)));
        int count = calls.size();
        check(child.getPackageStates() == filtered && calls.size() == count);
        immutable(() -> filtered.clear());
        var sibling = parent.filtered(1000, UserHandle.of(0));
        child.close(); closed(() -> child.getPackageStates());
        check(sibling.getPackageState("a") == a);
        check(parent.getPackageStates().get("a") == a);
        parent.close(); parent.close();
        closed(() -> parent.getPackageStates());
        closed(() -> parent.getDisabledSystemPackageStates());
        closed(() -> parent.getSharedUsers());
        closed(() -> sibling.getPackageState("a"));
        closed(() -> sibling.getPackageStates());
        // Original filtered() allows creation after close; access checks its parent.
        var after = parent.filtered(1000, UserHandle.of(0));
        closed(() -> after.getPackageStates()); after.close(); sibling.close();
        check(filtered.get("a") == a); // An already returned map stays usable.

        var art = PackageSnapshots.filtered(data, 19001, UserHandle.of(0), uncommitted);
        int before = calls.size();
        check(art.getPackageState("b") == uncommitted && calls.size() == before);
        check(art.getPackageStates().get("b") == uncommitted);
        check(calls.get(calls.size() - 1).equals("42:b:29002:19001:0"));
        art.close(); closed(() -> art.getPackageState("b"));
        var absent = PackageSnapshots.filtered(data, 19001, UserHandle.of(0), state("new", 19003));
        check(absent.getPackageState("new") != null);
        check(!absent.getPackageStates().containsKey("new")); absent.close();

        var next = new PackageSnapshots.Data(43, Map.of("b", uncommitted), Map.of(), Map.of(), visibility);
        var current = PackageSnapshots.unfiltered(next);
        check(current.getPackageStates().get("b") == uncommitted);
        var old = PackageSnapshots.unfiltered(data);
        check(old.getPackageStates().get("b") == b);
        var nextFiltered = PackageSnapshots.filtered(next, 19001, UserHandle.of(0), null);
        check(nextFiltered.getPackageState("b") == uncommitted);
        check(lookups.get(lookups.size() - 1).equals("43:b:19001:0"));
        nextFiltered.close();
        current.close(); old.close();
        var failures = new PackageSnapshots.Data(44, Map.of("a", a), Map.of(), Map.of(),
                new PackageSnapshots.Owner() {
                    public String getFilteredPackageName(long version, String name, int uid, int user) {
                        throw new IllegalArgumentException("owner failure");
                    }
                    public boolean shouldFilter(long version, PackageState pkg, int uid, int user) {
                        throw new IllegalArgumentException("owner failure");
                    }
                });
        var failure = PackageSnapshots.filtered(failures, 1000, UserHandle.of(0), null);
        for (Runnable get : List.<Runnable>of(() -> failure.getPackageStates(),
                () -> failure.getPackageState("a"))) {
            try { get.run(); throw new AssertionError("owner failure was swallowed"); }
            catch (IllegalArgumentException expected) { check("owner failure".equals(expected.getMessage())); }
        }
        failure.close();
        var incomplete = new PackageSnapshots.Data(42, Map.of("b", b), Map.of(), Map.of(), visibility);
        var mismatch = PackageSnapshots.filtered(incomplete, 1000, UserHandle.of(0), null);
        try { mismatch.getPackageState("a"); throw new AssertionError("owner/replica mismatch was hidden"); }
        catch (IllegalStateException expected) {
            check("owner returned a package outside the snapshot".equals(expected.getMessage()));
        }
        mismatch.close();
        System.out.println("SNAPSHOTS scopes identity immutability version visibility uncommitted errors");
    }
}
