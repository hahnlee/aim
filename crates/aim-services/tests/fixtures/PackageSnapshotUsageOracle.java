package dev.aim.server;

import java.util.Arrays;

/** Real native usage publication must refresh original usage without mutating old views. */
public final class PackageSnapshotUsageOracle {
    public static void verify(PackageSnapshots.Store store, PackageSnapshots.Source source,
            PackageSnapshots.Owner owner, Runnable actualUsagePublication, String packageName, int reason)
            throws Exception {
        try (var before = store.computer()) {
            var original = before.getPackageStateInternal(packageName, 1000);
            if (!(original instanceof PackageStateReplica oldReplica)) throw new AssertionError("actual native package replica missing");
            long[] oldTimes = original.getLastPackageUsageTime();
            long oldVersion = before.getVersion();
            long oldMetadata = before.getMetadataVersion();
            try (var lagging = new PackageSnapshots.Store(source, owner, true, () -> oldVersion)) {
                lagging.refresh();
                actualUsagePublication.run();
                try (var stale = lagging.computer()) {
                    if (stale.getVersion() != oldVersion) throw new AssertionError("controlled stale version hint advanced");
                }
                try (var refreshed = lagging.refreshedComputer()) {
                    if (refreshed.getVersion() <= oldVersion || refreshed.getMetadataVersion() != oldMetadata)
                        throw new AssertionError("forced package capture lost usage-only metadata epoch");
                }
            }
            try (var after = store.computer()) {
                var updated = after.getPackageStateInternal(packageName, 1000);
                if (!(updated instanceof PackageStateReplica newReplica)) throw new AssertionError("updated native package replica missing");
                long[] newTimes = updated.getLastPackageUsageTime();
                if (after.getVersion() <= oldVersion || after.getMetadataVersion() != oldMetadata
                        || newTimes[reason] == oldTimes[reason]) throw new AssertionError("actual usage publication was not captured");
                if (!Arrays.equals(original.getLastPackageUsageTime(), oldTimes)) throw new AssertionError("new usage mutated retained original PackageState");
                if (!Arrays.equals(newReplica.detachedSetting().getPkgState().getLastPackageUsageTimeInMills(), newTimes)) throw new AssertionError("detached original PackageSetting usage is stale");
                if (!Arrays.equals(oldReplica.detachedSetting().getPkgState().getLastPackageUsageTimeInMills(), oldTimes)) throw new AssertionError("detached old PackageSetting usage changed");
                if (!Arrays.equals(newReplica.getTransientState().getLastPackageUsageTimeInMills(), newTimes)) throw new AssertionError("transient original usage is stale");
                if (!Arrays.equals(oldReplica.getTransientState().getLastPackageUsageTimeInMills(), oldTimes)) throw new AssertionError("old transient usage changed");
                // Both constructors must return independent original mutable owners.
                var detached = newReplica.detachedSetting();
                detached.getPkgState().setLastPackageUsageTimeInMills(reason, -777);
                if (!Arrays.equals(newReplica.getLastPackageUsageTime(), newTimes)
                        || !Arrays.equals(original.getLastPackageUsageTime(), oldTimes)) throw new AssertionError("mutable detached usage escaped its owner");
            }
        }
    }
}
