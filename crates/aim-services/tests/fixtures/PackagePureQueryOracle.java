package dev.aim.server;

/** Real capture parity for pure visibility and application DTO queries. */
public final class PackagePureQueryOracle {
    private PackagePureQueryOracle() {}

    public static void verify(PackageSnapshots.Source source, PackageSnapshots.Owner owner,
            String name, long flags, int filterUid, int user, int comparisonUid,
            boolean filterUninstalled) throws Exception {
        try (var store = new PackageSnapshots.Store(source, owner, true)) {
            boolean same = store.queryComputer((computer, uid, pid) ->
                    computer.isSameApp(name, flags, comparisonUid, user, uid, pid));
            boolean canQuery = store.queryComputer((computer, uid, pid) ->
                    computer.canQueryPackage(filterUid, name, uid, pid));
            boolean filter = store.filterAppAccessQuery(name, filterUid, user, filterUninstalled);
            boolean uidFilter = store.queryComputer((computer, uid, pid) ->
                    computer.filterUidAccess(comparisonUid, filterUid));
            var app = store.queryComputer((computer, uid, pid) ->
                    computer.getApplicationInfo(name, flags, user, filterUid, uid, pid));
            try (var unexpected = store.computer()) {
                throw new AssertionError("pure query materialized Java package graph");
            } catch (IllegalStateException expected) {
                if (!"native package replica is unavailable".equals(expected.getMessage())) throw expected;
            }
            store.refresh();
            try (var full = store.computer()) {
                if (same != full.isSameApp(name, flags, comparisonUid, user)
                        || canQuery != full.canQueryPackage(filterUid, name)
                        || filter != full.filterAppAccess(name, filterUid, user, filterUninstalled)
                        || uidFilter != full.filterAppAccess(comparisonUid, filterUid)
                        || !java.util.Arrays.equals(parcel(app), parcel(full.getApplicationInfo(name, flags, filterUid, user))))
                    throw new AssertionError("pure query differs from full native Computer");
            }
        }
    }

    private static byte[] parcel(android.content.pm.ApplicationInfo value) {
        if (value == null) return null;
        var parcel = android.os.Parcel.obtain();
        try { value.writeToParcel(parcel, 0); return parcel.marshall(); }
        finally { parcel.recycle(); }
    }
}
