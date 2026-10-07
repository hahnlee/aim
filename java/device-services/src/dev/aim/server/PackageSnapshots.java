package dev.aim.server;

import android.os.UserHandle;
import android.os.RemoteException;
import java.io.IOException;
import com.android.server.pm.PackageManagerLocal;
import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.SharedUserApi;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;

/** Snapshot scopes for the C facade, after native replica capture. */
public final class PackageSnapshots {
    private PackageSnapshots() {}

    /** Computed by the native owner against the captured version and candidate. */
    public interface Owner {
        String getFilteredPackageName(long version, String name, int callingUid, int userId);
        boolean shouldFilter(long version, PackageState state, int callingUid, int userId);
    }

    @FunctionalInterface
    public interface Source {
        IPackageScanSnapshot capture() throws RemoteException;
    }

    /** Read from the native owner's read-only version page, without a Binder call. */
    @FunctionalInterface
    public interface VersionSource extends AutoCloseable {
        long currentVersion();
        @Override default void close() {}
    }

    /** One service-host lifetime; a new host Binder requires a new Store. */
    public static final class Store implements AutoCloseable {
        private final Source source;
        private final VersionSource versions;
        private final Owner owner;
        private final boolean crossUserSuspensions;
        private volatile Data current;
        private boolean closed;

        public Store(Source source, Owner owner, boolean crossUserSuspensions) {
            this(source, owner, crossUserSuspensions, null);
        }

        public Store(Source source, Owner owner, boolean crossUserSuspensions,
                VersionSource versions) {
            this.source = Objects.requireNonNull(source);
            this.versions = versions;
            this.owner = Objects.requireNonNull(owner);
            this.crossUserSuspensions = crossUserSuspensions;
        }

        public synchronized long refresh() throws RemoteException, IOException {
            if (closed) throw new IllegalStateException("package replica store is closed");
            var endpoint = source.capture();
            if (endpoint == null) throw new IOException("missing native snapshot endpoint");
            Data next = null;
            try (var lease = new PackageScanLease(endpoint)) {
                long version = lease.getVersion();
                if (current != null) {
                    if (version < current.version) throw new IOException("native snapshot version moved backwards");
                    if (version == current.version) return version;
                }
                next = lease.captureData(owner, crossUserSuspensions);
            } catch (RemoteException | IOException | RuntimeException failure) {
                if (next != null) {
                    try { next.close(); } catch (RuntimeException closeFailure) { failure.addSuppressed(closeFailure); }
                }
                throw failure;
            }
            Data previous = current;
            current = next;
            if (previous != null) previous.close();
            return next.version;
        }

        private synchronized Data capture() {
            if (closed) throw new IllegalStateException("package replica store is closed");
            Data data = current;
            if (data == null) throw new IllegalStateException("native package replica is unavailable");
            if (versions == null) return data;
            long observed = versions.currentVersion();
            if (observed < data.version)
                throw new IllegalStateException("native package version moved backwards");
            if (observed == data.version) return data;
            try {
                refresh();
            } catch (RemoteException failure) {
                throw failure.rethrowFromSystemServer();
            } catch (IOException failure) {
                throw new IllegalStateException("native package replica refresh failed", failure);
            }
            data = current;
            if (data.version < observed)
                throw new IllegalStateException("native package capture predates published version");
            return data;
        }

        public long getVersion() { return capture().version; }

        public synchronized ComputerSnapshot computer() { return new ComputerSnapshot(capture()); }

        @Override
        public synchronized void close() {
            if (closed) return;
            closed = true;
            Data previous = current;
            current = null;
            try { if (previous != null) previous.close(); }
            finally { if (versions != null) versions.close(); }
        }

        public synchronized PackageManagerLocal.UnfilteredSnapshot unfiltered() {
            return PackageSnapshots.unfiltered(capture());
        }

        public synchronized PackageManagerLocal.FilteredSnapshot filtered(int callingUid,
                UserHandle user, PackageState uncommitted) {
            return PackageSnapshots.filtered(capture(), callingUid, user, uncommitted);
        }
    }

    /** Records must be immutable and keep their identity for the same version. */
    public static final class Data implements AutoCloseable {
        private final long version;
        private final Map<String, PackageState> packages;
        private final Map<String, PackageState> disabled;
        private final Map<String, SharedUserApi> sharedUsers;
        private final Owner owner;
        private final IPackageComputer computer;
        private int references = 1;
        private boolean ownerClosed;

        public Data(long version, Map<String, PackageState> packages,
                Map<String, PackageState> disabled, Map<String, SharedUserApi> sharedUsers,
                Owner owner) {
            this(version, packages, disabled, sharedUsers, owner, null);
        }

        Data(long version, Map<String, PackageState> packages,
                Map<String, PackageState> disabled, Map<String, SharedUserApi> sharedUsers,
                Owner owner, IPackageComputer computer) {
            this.version = version;
            this.computer = computer;
            this.packages = copy(packages);
            this.disabled = copy(disabled);
            this.sharedUsers = copy(sharedUsers);
            this.owner = Objects.requireNonNull(owner);
        }

        synchronized void retain() {
            if (references == 0) throw new IllegalStateException("package query capture is released");
            references = Math.addExact(references, 1);
        }

        @Override
        public void close() {
            synchronized (this) {
                if (ownerClosed) return;
                ownerClosed = true;
            }
            release();
        }

        void release() {
            IPackageComputer endpoint;
            synchronized (this) {
                if (references == 0) throw new IllegalStateException("package query capture already released");
                endpoint = --references == 0 ? computer : null;
            }
            if (endpoint != null) {
                try { endpoint.close(); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
        }

        private static <T> Map<String, T> copy(Map<String, T> source) {
            Map<String, T> copy = new LinkedHashMap<>();
            source.forEach((key, value) -> copy.put(
                    Objects.requireNonNull(key), Objects.requireNonNull(value)));
            return Collections.unmodifiableMap(copy);
        }
    }

    public static PackageManagerLocal.UnfilteredSnapshot unfiltered(Data data) {
        return new Unfiltered(Objects.requireNonNull(data));
    }

    public static PackageManagerLocal.FilteredSnapshot filtered(Data data, int callingUid,
            UserHandle user, PackageState uncommitted) {
        return new Filtered(Objects.requireNonNull(data), callingUid, user, null, uncommitted);
    }

    // Lifetime and uncommitted-package behavior follow PackageManagerLocalImpl,
    // AOSP android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
    private abstract static class Scope implements AutoCloseable {
        protected Data data;
        private boolean closed;

        Scope(Data data) { data.retain(); this.data = data; }

        protected void checkClosed() {
            if (closed) throw new IllegalStateException("Snapshot already closed");
        }

        @Override
        public void close() {
            if (closed) return;
            closed = true;
            Data previous = data;
            data = null;
            previous.release();
        }
    }

    /** Typed internal query adapter retaining the same immutable graph as replica lookups. */
    public static final class ComputerSnapshot extends Scope {
        ComputerSnapshot(Data data) { super(data); }

        private IPackageComputer endpoint() {
            checkClosed();
            if (data.computer == null) throw new IllegalStateException("native package query capture unavailable");
            return data.computer;
        }

        public synchronized long getVersion() { checkClosed(); return data.version; }

        public synchronized PackageStateInternal getPackageStateInternal(String name) {
            checkClosed();
            return (PackageStateInternal) data.packages.get(name);
        }

        public synchronized android.content.pm.ApplicationInfo getApplicationInfo(String name,
                long flags, int filterCallingUid, int userId) {
            try {
                return endpoint().getApplicationInfo(name, flags, userId, filterCallingUid,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public synchronized android.content.pm.PackageInfo getPackageInfo(String name,
                long flags, int filterCallingUid, int userId) {
            try {
                return endpoint().getPackageInfo(name, flags, userId, filterCallingUid,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public synchronized boolean filterAppAccess(String name, int callingUid, int userId) {
            try { return endpoint().filterAppAccess(name, callingUid, userId); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        @Override public synchronized void close() { super.close(); }
    }

    private static final class Unfiltered extends Scope
            implements PackageManagerLocal.UnfilteredSnapshot {
        Unfiltered(Data data) { super(data); }

        @Override
        public PackageManagerLocal.FilteredSnapshot filtered(int callingUid, UserHandle user) {
            checkClosed();
            return new Filtered(data, callingUid, user, this, null);
        }

        @Override
        public Map<String, PackageState> getPackageStates() {
            checkClosed();
            return data.packages;
        }

        @Override
        public Map<String, PackageState> getDisabledSystemPackageStates() {
            checkClosed();
            return data.disabled;
        }

        @Override
        public Map<String, SharedUserApi> getSharedUsers() {
            checkClosed();
            return data.sharedUsers;
        }
    }

    private static final class Filtered extends Scope
            implements PackageManagerLocal.FilteredSnapshot {
        private final int callingUid;
        private final int userId;
        private final Unfiltered parent;
        private final PackageState uncommitted;
        private Map<String, PackageState> filtered;

        Filtered(Data data, int callingUid, UserHandle user, Unfiltered parent,
                PackageState uncommitted) {
            super(data);
            this.callingUid = callingUid;
            this.userId = user.getIdentifier();
            this.parent = parent;
            this.uncommitted = uncommitted;
        }

        @Override
        protected void checkClosed() {
            if (parent != null) parent.checkClosed();
            super.checkClosed();
        }

        @Override
        public PackageState getPackageState(String name) {
            checkClosed();
            if (uncommitted != null && name.equals(uncommitted.getPackageName())) {
                return uncommitted;
            }
            String resolved = data.owner.getFilteredPackageName(data.version, name, callingUid, userId);
            if (resolved == null) return null;
            PackageState state = data.packages.get(resolved);
            if (state == null) throw new IllegalStateException("owner returned a package outside the snapshot");
            return state;
        }

        @Override
        public Map<String, PackageState> getPackageStates() {
            checkClosed();
            if (filtered == null) {
                Map<String, PackageState> result = new LinkedHashMap<>();
                data.packages.forEach((name, original) -> {
                    PackageState state = uncommitted != null
                            && original.getPackageName().equals(uncommitted.getPackageName())
                            ? (PackageStateInternal) uncommitted : original;
                    if (!data.owner.shouldFilter(data.version, state, callingUid, userId)) {
                        result.put(name, state);
                    }
                });
                filtered = Collections.unmodifiableMap(result);
            }
            return filtered;
        }

        @Override
        public void close() {
            super.close();
            filtered = null;
        }
    }
}
