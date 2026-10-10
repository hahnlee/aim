package dev.aim.server;

import android.os.RemoteException;
import java.io.IOException;
import java.util.IdentityHashMap;
import java.util.Objects;

/** Private native PM transaction view used by the original permission owner. */
public final class NativeInstallPermissionScope implements InstallerPermissionBridge.CandidateScope {
    private static final ThreadLocal<IdentityHashMap<PackageSnapshots.Store, Frame>> scopes =
            ThreadLocal.withInitial(IdentityHashMap::new);
    private final PackageSnapshots.Store store;
    private final PackageSnapshots.Owner owner;
    private final boolean crossUserSuspensions;

    public NativeInstallPermissionScope(PackageSnapshots.Store store,
            PackageSnapshots.Owner owner, boolean crossUserSuspensions) {
        this.store = Objects.requireNonNull(store);
        this.owner = Objects.requireNonNull(owner);
        this.crossUserSuspensions = crossUserSuspensions;
    }

    /** PackageSnapshots.Store.capture calls this before consulting its version page. */
    public static PackageSnapshots.Data candidate(PackageSnapshots.Store store) {
        var frame = scopes.get().get(store);
        return frame == null ? null : frame.data;
    }

    @Override public AutoCloseable enter(IPackageScanSnapshot candidate, IPackageScanSnapshot previous)
            throws RemoteException, IOException {
        Bridge.enforceSystemUid();
        var current = scopes.get();
        if (current.containsKey(store)) throw new IllegalStateException("nested permission candidate scope");
        PackageSnapshots.Data data;
        try (var lease = new PackageScanLease(Objects.requireNonNull(candidate));
                var before = new PackageScanLease(Objects.requireNonNull(previous))) {
            // The native owner hands off two retained snapshots. Unrelated
            // usage publications may advance the live store during this read.
            if (lease.getVersion() != Math.addExact(before.getVersion(), 1))
                throw new IllegalStateException("permission candidate generation differs");
            data = lease.captureData(owner, crossUserSuspensions);
        }
        var frame = new Frame(store, data, Thread.currentThread());
        current.put(store, frame);
        return frame;
    }

    @Override public AutoCloseable restore(IPackageScanSnapshot snapshot, java.util.Set<String> excluded)
            throws RemoteException, IOException {
        Bridge.enforceSystemUid();
        var current = scopes.get();
        if (current.containsKey(store)) throw new IllegalStateException("nested permission rollback scope");
        PackageSnapshots.Data data;
        try (var lease = new PackageScanLease(Objects.requireNonNull(snapshot))) {
            data = lease.captureData(owner, crossUserSuspensions);
        }
        if (!excluded.isEmpty()) {
            var original = data;
            data = original.withoutPackages(excluded);
            original.close();
        }
        var frame = new Frame(store, data, Thread.currentThread());
        current.put(store, frame);
        return frame;
    }

    private static final class Frame implements AutoCloseable {
        private final PackageSnapshots.Store store;
        private final PackageSnapshots.Data data;
        private final Thread thread;
        private boolean closed;
        Frame(PackageSnapshots.Store store, PackageSnapshots.Data data, Thread thread) {
            this.store = store; this.data = data; this.thread = thread;
        }
        @Override public void close() {
            if (Thread.currentThread() != thread)
                throw new IllegalStateException("permission candidate closed on another thread");
            if (closed) return;
            var current = scopes.get();
            if (current.get(store) != this)
                throw new IllegalStateException("permission candidate scope ownership changed");
            current.remove(store);
            if (current.isEmpty()) scopes.remove();
            closed = true;
            data.close();
        }
    }
}
