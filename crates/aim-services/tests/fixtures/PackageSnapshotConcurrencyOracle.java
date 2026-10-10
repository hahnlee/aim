package dev.aim.server;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

/** Uses real native endpoints and an actual install publication supplied by the runner. */
public final class PackageSnapshotConcurrencyOracle {
    private static void await(CountDownLatch latch) {
        try {
            if (!latch.await(120, TimeUnit.SECONDS)) throw new AssertionError("snapshot concurrency owner did not complete");
        } catch (InterruptedException failure) {
            Thread.currentThread().interrupt();
            throw new AssertionError(failure);
        }
    }

    public static void verify(PackageSnapshots.Source realSource, PackageSnapshots.Owner realOwner,
            PackageSnapshots.VersionSource versions, Runnable actualPublish, String addedPackage) throws Exception {
        var entered = new CountDownLatch(1);
        var published = new CountDownLatch(1);
        var recursive = new CountDownLatch(1);
        var release = new CountDownLatch(1);
        var finished = new CountDownLatch(1);
        var calls = new AtomicInteger();
        var error = new AtomicReference<Throwable>();
        var holder = new PackageSnapshots.Store[1];
        var initial = new long[1];
        PackageSnapshots.Source delayed = () -> {
            if (Thread.holdsLock(holder[0])) throw new AssertionError("native capture RPC retained Store monitor");
            var endpoint = realSource.capture();
            if (calls.incrementAndGet() == 2) {
                entered.countDown();
                await(published);
                // Only this actual recursive call may retain the prior graph.
                if (holder[0].getVersion() != initial[0]) throw new AssertionError("same-thread refresh lost its retained capture");
                recursive.countDown();
                await(release);
            }
            return endpoint;
        };
        PackageSnapshots.VersionSource checkedVersions = new PackageSnapshots.VersionSource() {
            public long currentVersion() {
                if (Thread.holdsLock(holder[0])) throw new AssertionError("version owner retained Store monitor");
                return versions.currentVersion();
            }
            public void close() { versions.close(); }
        };
        try (var store = new PackageSnapshots.Store(delayed, realOwner, false, checkedVersions)) {
            holder[0] = store;
            initial[0] = store.refresh();
            try (var before = store.unfiltered()) {
                if (before.getPackageStates().containsKey(addedPackage)) throw new AssertionError("new package already present before actual publication");
            }
            var oldReader = new Thread(() -> {
                try { store.refresh(); }
                catch (Throwable failure) { error.set(failure); }
                finally { finished.countDown(); }
            }, "retained-native-snapshot");
            oldReader.start();
            try {
                await(entered);
                actualPublish.run();
                if (versions.currentVersion() <= initial[0]) throw new AssertionError("actual native publication did not advance");
                published.countDown();
                await(recursive);
                // This independent caller must capture the real installed graph,
                // even while the prior endpoint is retained on another thread.
                try (var after = store.unfiltered()) {
                    if (!after.getPackageStates().containsKey(addedPackage)) throw new AssertionError("concurrent packageAdded snapshot omitted the installed package");
                }
                long latest = store.getVersion();
                release.countDown();
                await(finished);
                oldReader.join();
                if (error.get() != null) throw new AssertionError("older retained capture failed", error.get());
                if (store.getVersion() < latest) throw new AssertionError("older capture replaced the newer graph");
                try (var retained = store.unfiltered()) {
                    if (!retained.getPackageStates().containsKey(addedPackage)) throw new AssertionError("older capture removed installed package");
                }
            } finally {
                published.countDown();
                release.countDown();
                oldReader.join(120000);
                if (oldReader.isAlive()) throw new AssertionError("snapshot worker remained alive");
            }
        }
    }
}
