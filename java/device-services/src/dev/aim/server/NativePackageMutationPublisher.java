package dev.aim.server;

import android.os.IBinder;
import android.os.RemoteException;
import java.io.IOException;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicBoolean;

/** Original mutator consumers operate on a retained native reservation's exact graph. */
public final class NativePackageMutationPublisher implements NativePackageStateMutation.Publisher {
    private final IPackageInternalHost host;
    private final PackageSnapshots.Owner owner;
    private final boolean crossUserSuspensions;

    public NativePackageMutationPublisher(IPackageInternalHost host, PackageSnapshots.Owner owner,
            boolean crossUserSuspensions) {
        this.host = Objects.requireNonNull(host);
        this.owner = Objects.requireNonNull(owner);
        this.crossUserSuspensions = crossUserSuspensions;
    }
    @Override public NativePackageStateMutation.Reservation reserve() {
        IPackageMutationReservation reservation;
        try { reservation = NativePackageCapabilities.attach(Objects.requireNonNull(host.reserveMutation(), "native mutation reservation unavailable")); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        return acquire(reservation);
    }
    private NativePackageStateMutation.Reservation acquire(IPackageMutationReservation endpoint) {
        PackageScanLease lease = null;
        PackageSnapshots.Data data = null;
        PackageSnapshots.ComputerSnapshot scope = null;
        try {
            IPackageScanSnapshot snapshot = Objects.requireNonNull(endpoint.getSnapshot(), "reserved native graph unavailable");
            lease = new PackageScanLease(snapshot);
            long version = lease.getVersion();
            data = lease.captureData(owner, crossUserSuspensions);
            scope = new PackageSnapshots.ComputerSnapshot(data);
            if (scope.getVersion() != version) throw new IllegalStateException("mutation capture version differs");
            int sequence = endpoint.getChangedPackagesSequence();
            Retained result = new Retained(endpoint, lease, data, scope, version, sequence);
            result.attachDeath();
            return result;
        } catch (RemoteException | IOException | RuntimeException failure) {
            closePartial(endpoint, lease, data, scope, failure);
            throw new IllegalStateException("native mutation reservation capture failed", failure);
        }
    }
    private static void closePartial(IPackageMutationReservation endpoint, PackageScanLease lease,
            PackageSnapshots.Data data, PackageSnapshots.ComputerSnapshot scope, Throwable failure) {
        if (scope != null) try { scope.close(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
        if (data != null) try { data.close(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
        if (lease != null) try { lease.close(); } catch (RemoteException | RuntimeException cleanup) { failure.addSuppressed(cleanup); }
        try { endpoint.close(); } catch (RemoteException | RuntimeException cleanup) { failure.addSuppressed(cleanup); }
    }
    private static final class Retained implements NativePackageStateMutation.Reservation {
        private final IPackageMutationReservation endpoint;
        private final PackageScanLease lease;
        private final PackageSnapshots.Data data;
        private final PackageSnapshots.ComputerSnapshot scope;
        private final long version;
        private final int sequence;
        private final AtomicBoolean closed = new AtomicBoolean();
        private final AtomicBoolean dead = new AtomicBoolean();
        private final AtomicBoolean published = new AtomicBoolean();
        private final java.util.concurrent.atomic.AtomicReference<RuntimeException> cleanupFailure = new java.util.concurrent.atomic.AtomicReference<>();
        private final IBinder.DeathRecipient death = () -> {
            dead.set(true);
            try { close(); } catch (RuntimeException failure) { cleanupFailure.compareAndSet(null, failure); }
        };
        private boolean linked;
        Retained(IPackageMutationReservation endpoint, PackageScanLease lease,
                PackageSnapshots.Data data, PackageSnapshots.ComputerSnapshot scope,
                long version, int sequence) {
            this.endpoint = endpoint; this.lease = lease; this.data = data;
            this.scope = scope; this.version = version; this.sequence = sequence;
        }
        void attachDeath() throws RemoteException {
            endpoint.asBinder().linkToDeath(death, 0);
            linked = true;
            requireOpen();
        }
        private void requireOpen() {
            if (dead.get()) throw new IllegalStateException("native mutation reservation owner died", cleanupFailure.get());
            if (closed.get()) throw new IllegalStateException("native mutation reservation closed", cleanupFailure.get());
        }
        @Override public PackageSnapshots.ComputerSnapshot snapshot() { requireOpen(); return scope; }
        @Override public int changedPackagesSequence() { requireOpen(); return sequence; }
        @Override public int[] disabledUserAliases(String packageName) {
            requireOpen(); Objects.requireNonNull(packageName);
            if (scope.getDisabledSystemPackage(packageName) == null)
                throw new IllegalArgumentException("package has no disabled factory state");
            try {
                int[] aliases = Objects.requireNonNull(endpoint.getDisabledUserAliases(packageName), "native factory alias owner unavailable");
                requireOpen();
                java.util.HashSet<Integer> seen = new java.util.HashSet<>();
                var factory = scope.getDisabledSystemPackage(packageName);
                var active = scope.getPackageStateInternal(packageName);
                for (int user : aliases) {
                    if (user < 0 || !seen.add(user) || active == null
                            || active.getUserStates().get(user) == null
                            || factory.getUserStates().get(user) == null)
                        throw new IllegalStateException("native factory alias has no retained active user owner");
                }
                return aliases.clone();
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        @Override public void publish(long expectedVersion, byte[] mutationRecord) {
            requireOpen(); Objects.requireNonNull(mutationRecord);
            if (expectedVersion != version || scope.getVersion() != version)
                throw new IllegalStateException("mutation publication version differs from reservation");
            if (!published.compareAndSet(false, true)) throw new IllegalStateException("reservation already published");
            try { endpoint.publish(expectedVersion, mutationRecord.clone()); requireOpen(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        @Override public void close() {
            if (!closed.compareAndSet(false, true)) {
                RuntimeException previous = cleanupFailure.get();
                if (previous != null) throw previous;
                return;
            }
            RuntimeException failure = null;
            try { scope.close(); } catch (RuntimeException cleanup) { failure = cleanup; }
            try { data.close(); } catch (RuntimeException cleanup) { failure = append(failure, cleanup); }
            try { lease.close(); } catch (RemoteException cleanup) { failure = append(failure, cleanup.rethrowFromSystemServer()); }
            catch (RuntimeException cleanup) { failure = append(failure, cleanup); }
            try { endpoint.close(); } catch (RemoteException cleanup) { failure = append(failure, cleanup.rethrowFromSystemServer()); }
            catch (RuntimeException cleanup) { failure = append(failure, cleanup); }
            if (linked) {
                try { endpoint.asBinder().unlinkToDeath(death, 0); }
                catch (RuntimeException cleanup) { failure = append(failure, cleanup); }
            }
            if (failure != null) { cleanupFailure.compareAndSet(null, failure); throw failure; }
        }
        private static RuntimeException append(RuntimeException first, RuntimeException next) {
            if (first == null) return next;
            first.addSuppressed(next); return first;
        }
    }
}
