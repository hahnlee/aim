package dev.aim.server;

/** Actual original public mutations must isolate retained native user records. */
public final class PackageSnapshotUserDeltaOracle {
    private PackageSnapshotUserDeltaOracle() {}

    public static void verify(PackageSnapshots.Source source, PackageSnapshots.Owner owner,
            PackageSnapshots.VersionSource versions, android.content.pm.IPackageManager manager, String packageName)
            throws Exception {
        var baseline = source.capture();
        long comparison = baseline.getMetadataComparisonId();
        if (comparison <= 0) throw new AssertionError("native comparison capability missing");
        boolean sharedTarget = false;
        try (var store = new PackageSnapshots.Store(source, owner, true, versions)) {
            store.refresh();
            try (var before = store.computer()) {
                var original = java.util.Objects.requireNonNull(before.getPackageStateInternal(packageName, 1000));
                sharedTarget = original.hasSharedUser();
                var oldUser = original.getUserStateOrDefault(0);
                boolean stopped = oldUser.isStopped();
                var code = java.util.Objects.requireNonNull(original.getAndroidPackage());
                if (code.getActivities().isEmpty()) throw new AssertionError("actual package activity missing");
                var parsed = code.getActivities().get(0);
                var component = new android.content.ComponentName(parsed.getPackageName(), parsed.getName());
                boolean disabled = oldUser.getDisabledComponents().contains(component.getClassName());
                manager.setPackageStoppedState(original.getPackageName(), !stopped, 0);
                try (var changed = store.computer()) {
                    var update = java.util.Objects.requireNonNull(changed.getPackageStateInternal(original.getPackageName(), 1000));
                    if (changed.getVersion() <= before.getVersion()
                            || update.getUserStateOrDefault(0).isStopped() == stopped
                            || original.getUserStateOrDefault(0).isStopped() != stopped)
                        throw new AssertionError("stopped publication changed wrong user snapshot");
                    unchangedCode(original, update);
                    sharedMember(changed, update);
                }
                // Original DONT_KILL_APP is 1; mutate only this disposable guest's genuine component.
                manager.setComponentEnabledSetting(component, disabled ? 1 : 2, 1, 0, "android");
                try (var changed = store.computer()) {
                    var update = java.util.Objects.requireNonNull(changed.getPackageStateInternal(original.getPackageName(), 1000));
                    if (update.getUserStateOrDefault(0).getDisabledComponents().contains(component.getClassName()) == disabled
                            || original.getUserStateOrDefault(0).getDisabledComponents().contains(component.getClassName()) != disabled)
                        throw new AssertionError("component publication changed retained user snapshot");
                    unchangedCode(original, update);
                    sharedMember(changed, update);
                }
                var latest = source.capture();
                try {
                    byte[] changes = latest.getChangedUsersForMetadataBase(comparison);
                    if (changes == null) throw new AssertionError("retained actual user-change comparison requires full fallback");
                    var record = android.os.Parcel.obtain();
                    try {
                        record.unmarshall(changes, 0, changes.length); record.setDataPosition(0);
                        if (record.readLong() != latest.getVersion()
                                || record.readLong() != latest.getMetadataVersion()
                                || record.readInt() != 1
                                || !original.getPackageName().equals(record.readString())
                                || record.readBoolean() || record.dataAvail() != 0)
                            throw new AssertionError("user comparison returned wrong current record keys");
                    } finally { record.recycle(); }
                    baseline.close(); baseline = null;
                    if (latest.getChangedUsersForMetadataBase(comparison) != null)
                        throw new AssertionError("expired comparison capability remained usable");
                } finally { latest.close(); }
            }
        } finally { if (baseline != null) baseline.close(); }
        System.out.println("NATIVE_USER_DELTA sharedUid="
                + (sharedTarget ? "member_rebound_verified" : "not_applicable"));
    }

    private static void unchangedCode(com.android.server.pm.pkg.PackageState before,
            com.android.server.pm.pkg.PackageState after) {
        if (before.getVersionCode() != after.getVersionCode()
                || !before.getPath().equals(after.getPath())
                || !before.getAndroidPackage().getPackageName().equals(after.getAndroidPackage().getPackageName()))
            throw new AssertionError("user mutation altered accepted code");
    }

    private static void sharedMember(PackageSnapshots.ComputerSnapshot computer,
            com.android.server.pm.pkg.PackageState state) {
        if (!state.hasSharedUser()) return;
        var group = java.util.Objects.requireNonNull(computer.getSharedUserApi(state.getSharedUserAppId()));
        for (var member : group.getPackageStates()) {
            if (member.getPackageName().equals(state.getPackageName())) {
                if (member != state) throw new AssertionError("shared UID member retained old user record");
                return;
            }
        }
        throw new AssertionError("actual shared UID member missing");
    }
}
