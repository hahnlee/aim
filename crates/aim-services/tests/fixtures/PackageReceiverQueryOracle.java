package dev.aim.server;

/** Compare the narrow resolver with the same native Computer and capture lifecycle. */
public final class PackageReceiverQueryOracle {
    private PackageReceiverQueryOracle() {}

    public static void verify(PackageSnapshots.Source source, PackageSnapshots.Owner owner,
            android.content.Intent intent, String type, long flags, int user,
            int filterUid, int filterPid, boolean start) throws Exception {
        var captures = new java.util.concurrent.atomic.AtomicInteger();
        var observed = new java.util.concurrent.atomic.AtomicReference<PackageSnapshots.Store>();
        PackageSnapshots.Source counted = () -> {
            if (Thread.holdsLock(observed.get())) throw new AssertionError("query capture holds Store monitor");
            captures.incrementAndGet();
            return source.capture();
        };
        var store = new PackageSnapshots.Store(counted, owner, true);
        observed.set(store);
        try {
            var direct = store.queryIntentReceiversQuery(intent, type, flags, filterUid, filterPid, user, start);
            if (captures.get() != 1) throw new AssertionError("receiver query did not capture exactly one endpoint");
            // No refresh was performed: a pure native query leaves the Java graph uninitialized.
            try (var unexpected = store.computer()) {
                throw new AssertionError("query materialized package graph");
            } catch (IllegalStateException expected) {
                if (!"native package replica is unavailable".equals(expected.getMessage())) throw expected;
            }
            store.refresh();
            try (var scope = store.computer()) {
                var complete = scope.queryIntentReceiversInternal(intent, type, flags, filterUid, filterPid, user, start);
                if (!java.util.Arrays.equals(parcel(direct), parcel(complete)))
                    throw new AssertionError("receiver resolution differs from full native Computer capture");
            }
        } finally { store.close(); }
        int closedCaptures = captures.get();
        try {
            store.queryIntentReceiversQuery(intent, type, flags, filterUid, filterPid, user, start);
            throw new AssertionError("closed receiver query was accepted");
        } catch (IllegalStateException expected) {}
        if (captures.get() != closedCaptures) throw new AssertionError("closed query captured an endpoint");
    }

    private static byte[] parcel(java.util.List<android.content.pm.ResolveInfo> value) {
        if (value == null) return null;
        var parcel = android.os.Parcel.obtain();
        try { parcel.writeInt(value.size()); for (var item : value) { parcel.writeBoolean(item != null); if (item != null) item.writeToParcel(parcel, 0); } return parcel.marshall(); }
        finally { parcel.recycle(); }
    }
}
