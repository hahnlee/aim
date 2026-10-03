package dev.aim.server;

import android.os.UserHandle;
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

    /** Records must be immutable and keep their identity for the same version. */
    public static final class Data {
        private final long version;
        private final Map<String, PackageState> packages;
        private final Map<String, PackageState> disabled;
        private final Map<String, SharedUserApi> sharedUsers;
        private final Owner owner;

        public Data(long version, Map<String, PackageState> packages,
                Map<String, PackageState> disabled, Map<String, SharedUserApi> sharedUsers,
                Owner owner) {
            this.version = version;
            this.packages = copy(packages);
            this.disabled = copy(disabled);
            this.sharedUsers = copy(sharedUsers);
            this.owner = Objects.requireNonNull(owner);
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

        Scope(Data data) { this.data = data; }

        protected void checkClosed() {
            if (closed) throw new IllegalStateException("Snapshot already closed");
        }

        @Override
        public void close() {
            closed = true;
            data = null;
        }
    }

    private static final class Unfiltered extends Scope
            implements PackageManagerLocal.UnfilteredSnapshot {
        Unfiltered(Data data) { super(data); }

        @Override
        public PackageManagerLocal.FilteredSnapshot filtered(int callingUid, UserHandle user) {
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
