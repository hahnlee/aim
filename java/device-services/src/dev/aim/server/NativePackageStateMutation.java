package dev.aim.server;

import com.android.server.pm.PackageSetting;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.mutate.PackageStateMutator;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;
import java.util.function.Consumer;

/** Executes original mutator consumers on detached originals before native atomic publication. */
public final class NativePackageStateMutation {
    /** Retains the native write reservation until commit or close; never holds a Binder callback lock. */
    public interface Reservation extends AutoCloseable {
        PackageSnapshots.ComputerSnapshot snapshot();
        int changedPackagesSequence();
        int[] disabledUserAliases(String packageName);
        void publish(long expectedVersion, byte[] mutationRecord);
        @Override void close();
    }
    public interface Publisher { Reservation reserve(); }
    private final Object writeLock = new Object();
    private final Publisher publisher;

    public NativePackageStateMutation(Publisher publisher) {
        this.publisher = Objects.requireNonNull(publisher);
    }

    public PackageStateMutator.Result commit(PackageStateMutator.InitialState initial,
            Consumer<PackageStateMutator> consumer) {
        Objects.requireNonNull(consumer);
        synchronized (writeLock) {
            try (Reservation reservation = Objects.requireNonNull(publisher.reserve())) {
                var snapshot = Objects.requireNonNull(reservation.snapshot());
                var active = copy(snapshot.getPackageStates());
                var disabled = copy(snapshot.getDisabledSystemPackageStates());
                restoreUserAliases(reservation, active, disabled);
                var mutator = new PackageStateMutator(active::get, disabled::get);
                var result = mutator.generateResult(initial, reservation.changedPackagesSequence());
                if (result != PackageStateMutator.Result.SUCCESS) return result;
                // A failed consumer leaves the native owner untouched. The objects handed to it
                // are independent originals and cannot mutate a retained read generation.
                consumer.accept(mutator);
                mutator.onFinished();
                reservation.publish(snapshot.getVersion(), com.android.server.pm.NativePackageMutationRecord.encode(snapshot.getVersion(), active, disabled));
                return PackageStateMutator.Result.SUCCESS;
            }
        }
    }

    public void forEachPackageSetting(Consumer<PackageSetting> action) {
        Objects.requireNonNull(action);
        synchronized (writeLock) {
            try (Reservation reservation = Objects.requireNonNull(publisher.reserve())) {
                var snapshot = Objects.requireNonNull(reservation.snapshot());
                var active = copy(snapshot.getPackageStates());
                var disabled = copy(snapshot.getDisabledSystemPackageStates());
                restoreUserAliases(reservation, active, disabled);
                for (PackageSetting setting : active.values()) action.accept(setting);
                reservation.publish(snapshot.getVersion(), com.android.server.pm.NativePackageMutationRecord.encode(snapshot.getVersion(), active, disabled));
            }
        }
    }

    public void forEachInstalledPackage(Consumer<com.android.server.pm.pkg.AndroidPackage> action,
            int userId) {
        Objects.requireNonNull(action);
        try (Reservation reservation = Objects.requireNonNull(publisher.reserve())) {
            for (var state : reservation.snapshot().getPackageStates().values()) {
                if (state.getPkg() != null && state.getUserStateOrDefault(userId).isInstalled())
                    action.accept(state.getPkg());
            }
        }
    }

    public void overlays(int user, android.util.ArrayMap<String, android.content.pm.overlay.OverlayPaths> pending,
            java.util.Set<String> updated, java.util.Set<String> invalid, NativeSystemOverlayOwner system) {
        Objects.requireNonNull(pending); Objects.requireNonNull(updated); Objects.requireNonNull(invalid);
        Objects.requireNonNull(system);
        synchronized (writeLock) {
            try (Reservation reservation = Objects.requireNonNull(publisher.reserve())) {
                var snapshot = Objects.requireNonNull(reservation.snapshot());
                var active = copy(snapshot.getPackageStates());
                var disabled = copy(snapshot.getDisabledSystemPackageStates());
                restoreUserAliases(reservation, active, disabled);
                var dependentChanges = new LinkedHashMap<String, Map<String, java.util.Set<String>>>();
                for (int i = 0; i < pending.size(); i++) {
                    String name = pending.keyAt(i); var paths = pending.valueAt(i);
                    var state = snapshot.getPackageStateInternal(name);
                    var code = state == null ? null : state.getPkg();
                    if (name == null || code == null) {
                        android.util.Slog.e("PackageManager", "failed to find package " + name);
                        invalid.add(name); continue;
                    }
                    var old = state.getUserStateOrDefault(user).getOverlayPaths();
                    if (Objects.equals(old, paths)) continue;
                    if (code.getLibraryNames() != null) for (String library : code.getLibraryNames()) {
                        var info = snapshot.getSharedLibraryInfo(library, -1);
                        if (info == null) continue;
                        var dependents = snapshot.getPackagesUsingSharedLibrary(info, 0, 1000, user).first;
                        if (dependents == null) continue;
                        for (var dependent : dependents) {
                            var dependentState = snapshot.getPackageStateInternal(dependent.getPackageName());
                            if (dependentState == null) continue;
                            if (canSet(dependentState.getUserStateOrDefault(user).getSharedLibraryOverlayPaths().get(library), paths)) {
                                String changed = dependent.getPackageName();
                                dependentChanges.computeIfAbsent(name, unused -> new LinkedHashMap<>())
                                    .computeIfAbsent(library, unused -> new java.util.LinkedHashSet<>()).add(changed);
                                updated.add(changed);
                            }
                        }
                    }
                    if (canSet(old, paths)) updated.add(name);
                }
                for (int i = 0; i < pending.size(); i++) {
                    String name = pending.keyAt(i); var paths = pending.valueAt(i);
                    if (!updated.contains(name)) continue;
                    PackageSetting target = active.get(name);
                    if (target != null) target.getOrCreateUserState(user).setOverlayPaths(paths);
                    var dependents = dependentChanges.get(name);
                    if (dependents == null) continue;
                    dependents.forEach((library, names) -> names.forEach(dependent -> {
                        PackageSetting setting = active.get(dependent);
                        if (setting != null) setting.getOrCreateUserState(user).setSharedLibraryOverlayPaths(library, paths);
                    }));
                }
                reservation.publish(snapshot.getVersion(), com.android.server.pm.NativePackageMutationRecord.encode(snapshot.getVersion(), active, disabled));
            }
        }
        if (user == 0) for (int i = 0; i < pending.size(); i++) system.update(pending.keyAt(i), pending.valueAt(i));
    }
    private static boolean canSet(android.content.pm.overlay.OverlayPaths old,
            android.content.pm.overlay.OverlayPaths value) {
        if (Objects.equals(old, value)) return false;
        if ((old == null && value.isEmpty()) || (value == null && old.isEmpty())) return false;
        return true;
    }

    @SuppressWarnings("unchecked")
    private static void restoreUserAliases(Reservation reservation, Map<String, PackageSetting> active,
            Map<String, PackageSetting> disabled) {
        for (var entry : disabled.entrySet()) {
            int[] aliases = Objects.requireNonNull(reservation.disabledUserAliases(entry.getKey()),
                    "disabled original user alias owner unavailable");
            PackageSetting current = active.get(entry.getKey());
            var users = (android.util.SparseArray<com.android.server.pm.pkg.PackageUserStateImpl>)
                    (Object) entry.getValue().getUserStates();
            for (int user : aliases) {
                if (current == null || current.getUserStates().get(user) == null || users.get(user) == null)
                    throw new IllegalStateException("disabled user alias lacks an original owner");
                users.put(user, current.getOrCreateUserState(user));
            }
        }
    }

    private static Map<String, PackageSetting> copy(Map<String, PackageStateInternal> source) {
        var result = new LinkedHashMap<String, PackageSetting>();
        source.forEach((name, state) -> {
            PackageSetting original;
            if (state instanceof PackageStateReplica replica) original = replica.detachedSetting();
            else throw new IllegalStateException("package setting is outside the captured original owner");
            result.put(name, original);
        });
        return result;
    }
}
