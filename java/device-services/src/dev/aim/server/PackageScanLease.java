package dev.aim.server;

import android.os.Parcel;
import android.os.RemoteException;
import java.io.IOException;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;

/** Private code transport bound to one native capture, before replica construction. */
public final class PackageScanLease implements AutoCloseable {
    private static final int CHUNK = 64 * 1024;
    private final IPackageScanSnapshot endpoint;
    private final long version;
    private final Map<String, PackageCode> active = new HashMap<>();
    private final Map<String, PackageCode> disabled = new HashMap<>();
    private final Map<String, PackageUsageState> usage = new HashMap<>();
    private final Map<String, PackageSeInfoState> seinfo = new HashMap<>();
    private final Map<String, PackageLibraryState> libraries = new HashMap<>();
    private final Map<String, PackageSigningState> signing = new HashMap<>();
    private final Map<String, PackageSigningState> factorySigning = new HashMap<>();
    private record SettingKey(String name, boolean factory) {}
    private final Map<SettingKey, PackageSettingData> settings = new HashMap<>();
    private record UserKey(String name, boolean factory, int user) {}
    private final Map<UserKey, PackageUserStateData> users = new HashMap<>();
    private record ReplicaKey(UserKey user, boolean crossUserSuspensions) {}
    private final Map<ReplicaKey, PackageUserStateReplica> userReplicas = new HashMap<>();
    private boolean closed;

    public PackageScanLease(IPackageScanSnapshot endpoint) throws RemoteException {
        this.endpoint = Objects.requireNonNull(endpoint);
        version = endpoint.getVersion();
    }

    public long getVersion() { return version; }

    public synchronized PackageCode getCode(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        Map<String, PackageCode> cache = factory ? disabled : active;
        if (cache.containsKey(name)) return cache.get(name);
        int length = endpoint.getCodeLength(name, factory);
        if (length == -1) {
            cache.put(name, null);
            return null;
        }
        if (length <= 0) throw new IOException("invalid package code length");
        byte[] bytes = new byte[length];
        int offset = 0;
        while (offset < length) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getCodeChunk(name, factory, offset, requested);
            if (chunk == null || chunk.length != requested) {
                throw new IOException("incomplete package code chunk");
            }
            System.arraycopy(chunk, 0, bytes, offset, requested);
            offset += requested;
        }
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            PackageCode code = PackageCode.CREATOR.createFromParcel(parcel);
            if (parcel.dataAvail() != 0 || code.getVersion() != version
                    || !code.getPackageName().equals(name)) {
                throw new IOException("package code capture mismatch");
            }
            cache.put(name, code);
            return code;
        } finally {
            parcel.recycle();
        }
    }

    public synchronized PackageUsageState getUsage(String name) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        if (usage.containsKey(name)) return usage.get(name);
        byte[] bytes = endpoint.getUsage(name);
        if (bytes == null) {
            usage.put(name, null);
            return null;
        }
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            PackageUsageState state = PackageUsageState.CREATOR.createFromParcel(parcel);
            if (parcel.dataAvail() != 0 || state.getVersion() != version
                    || !state.getPackageName().equals(name)) {
                throw new IOException("package usage capture mismatch");
            }
            usage.put(name, state);
            return state;
        } finally { parcel.recycle(); }
    }

    @Override
    public synchronized void close() throws RemoteException {
        if (closed) return;
        endpoint.close();
        closed = true;
        active.clear();
        disabled.clear();
        usage.clear();
        seinfo.clear();
        libraries.clear();
        signing.clear();
        factorySigning.clear();
        users.clear();
        settings.clear();
        userReplicas.clear();
    }

    public synchronized PackageSigningState getSigningState(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        Map<String, PackageSigningState> cache = factory ? factorySigning : signing;
        if (cache.containsKey(name)) return cache.get(name);
        byte[] bytes = endpoint.getSigningState(name, factory);
        if (bytes == null) { cache.put(name, null); return null; }
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            PackageSigningState state = PackageSigningState.CREATOR.createFromParcel(parcel);
            if (parcel.dataAvail() != 0 || state.getVersion() != version
                    || !state.getPackageName().equals(name) || state.isDisabled() != factory) {
                throw new IOException("package signing capture mismatch");
            }
            // Reject invalid certificates before they can become a cached replica input.
            state.getPackageSigningDetails();
            state.getSharedSigningDetails();
            cache.put(name, state);
            return state;
        } finally { parcel.recycle(); }
    }

    public synchronized PackageSeInfoState getSeInfo(String name) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        if (seinfo.containsKey(name)) return seinfo.get(name);
        byte[] bytes = endpoint.getSeInfo(name);
        if (bytes == null) {
            seinfo.put(name, null);
            return null;
        }
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            PackageSeInfoState state = PackageSeInfoState.CREATOR.createFromParcel(parcel);
            if (parcel.dataAvail() != 0 || state.getVersion() != version
                    || !state.getPackageName().equals(name)) {
                throw new IOException("package seInfo capture mismatch");
            }
            seinfo.put(name, state);
            return state;
        } finally { parcel.recycle(); }
    }

    public synchronized PackageLibraryState getLibraries(String name)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        String key = name;
        if (libraries.containsKey(key)) return libraries.get(key);
        int length = endpoint.getLibraryStateLength(name);
        if (length == -1) { libraries.put(key, null); return null; }
        if (length <= 0) throw new IOException("invalid package library length");
        byte[] bytes = new byte[length];
        for (int offset = 0; offset < length;) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getLibraryStateChunk(name, offset, requested);
            if (chunk == null || chunk.length != requested) throw new IOException("incomplete package library chunk");
            System.arraycopy(chunk, 0, bytes, offset, requested);
            offset += requested;
        }
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            PackageLibraryState state = PackageLibraryState.read(in);
            if (in.dataAvail() != 0 || state.getVersion() != version || !state.getPackageName().equals(name)) {
                throw new IOException("package library capture mismatch");
            }
            libraries.put(key, state);
            return state;
        } finally { in.recycle(); }
    }
    public synchronized PackageSettingData getSetting(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        SettingKey key = new SettingKey(name, factory);
        if (settings.containsKey(key)) return settings.get(key);
        int length = endpoint.getSettingLength(name, factory);
        if (length == -1) { settings.put(key, null); return null; }
        if (length <= 0) throw new IOException("invalid package setting length");
        byte[] bytes = new byte[length];
        for (int offset = 0; offset < length;) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getSettingChunk(name, factory, offset, requested);
            if (chunk == null || chunk.length != requested) throw new IOException("incomplete package setting chunk");
            System.arraycopy(chunk, 0, bytes, offset, requested);
            offset += requested;
        }
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            PackageSettingData state = PackageSettingData.read(in);
            if (in.dataAvail() != 0 || state.getVersion() != version || !state.getPackageName().equals(name)
                    || state.isFactory() != factory) {
                throw new IOException("package setting capture mismatch");
            }
            settings.put(key, state);
            return state;
        } finally { in.recycle(); }
    }
    /** Detached original metadata and every explicit sparse user entry in this capture. */
    public synchronized com.android.server.pm.PackageSetting newSettingWithUsers(String name, boolean factory,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        PackageSettingData data = getSetting(name, factory);
        if (data == null) return null;
        int[] ids = endpoint.getUserStateIds(name, factory);
        if (ids == null) throw new IOException("missing package user inventory");
        var inputs = new java.util.ArrayList<PackageUserStateData>(ids.length);
        int previous = -1;
        for (int id : ids) {
            if (id <= previous) throw new IOException("invalid sparse user inventory");
            previous = id;
            PackageUserStateData user = getUserState(name, factory, id);
            if (user == null) throw new IOException("missing explicit package user state");
            inputs.add(user);
        }
        return com.android.server.pm.CapturedPackageSetting.withUsers(data, inputs, version, factory, crossUserSuspensions);
    }

    /** Collected active code, saved owners and finalized dependency metadata. */
    public synchronized com.android.server.pm.PackageSetting newScannedSetting(String name,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        var setting = newSettingWithUsers(name, false, crossUserSuspensions);
        if (setting == null) return null;
        PackageCode code = getCode(name, false);
        PackageSigningState saved = getSigningState(name, false);
        PackageUsageState times = getUsage(name);
        PackageSeInfoState labels = getSeInfo(name);
        PackageLibraryState dependencies = getLibraries(name);
        if (code == null || saved == null || times == null || labels == null || dependencies == null) {
            throw new IOException("missing collected package owner");
        }
        PackageObjects.restoreCollectedCode(setting, code, version, false);
        PackageObjects.restoreSavedSigning(setting, saved, version, false);
        PackageObjects.restoreUsage(setting, times, version);
        PackageObjects.restoreSeInfo(setting, labels, version);
        PackageObjects.restoreLibraries(setting, dependencies, version);
        return setting;
    }

    public synchronized PackageUserStateData getUserState(String name, boolean factory, int user)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        if (user < 0) throw new IllegalArgumentException("invalid user id");
        UserKey key = new UserKey(name, factory, user);
        if (users.containsKey(key)) return users.get(key);
        int length = endpoint.getUserStateLength(name, factory, user);
        if (length == -1) { users.put(key, null); return null; }
        if (length <= 0) throw new IOException("invalid user state length");
        byte[] bytes = new byte[length];
        for (int offset = 0; offset < length;) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getUserStateChunk(name, factory, user, offset, requested);
            if (chunk == null || chunk.length != requested) throw new IOException("incomplete user state chunk");
            System.arraycopy(chunk, 0, bytes, offset, requested);
            offset += requested;
        }
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            PackageUserStateData state = PackageUserStateData.read(in);
            if (in.dataAvail() != 0 || state.getVersion() != version || !state.getPackageName().equals(name)
                    || state.isFactory() != factory || state.getUserId() != user) {
                throw new IOException("package user state capture mismatch");
            }
            users.put(key, state);
            return state;
        } finally { in.recycle(); }
    }
    public synchronized PackageUserStateReplica getUserStateReplica(String name, boolean factory, int user,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        ReplicaKey key = new ReplicaKey(new UserKey(Objects.requireNonNull(name), factory, user), crossUserSuspensions);
        if (userReplicas.containsKey(key)) return userReplicas.get(key);
        PackageUserStateData data = getUserState(name, factory, user);
        PackageUserStateReplica replica = data == null ? null : new PackageUserStateReplica(data, crossUserSuspensions);
        userReplicas.put(key, replica);
        return replica;
    }

}
