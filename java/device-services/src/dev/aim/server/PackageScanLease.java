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
    private final Map<SettingKey, PackageTransientState> transientStates = new HashMap<>();
    private final Map<SettingKey, Integer> hiddenApiPolicies = new HashMap<>();
    private record UserKey(String name, boolean factory, int user) {}
    private final Map<UserKey, PackageUserStateData> users = new HashMap<>();
    private record ReplicaKey(UserKey user, boolean crossUserSuspensions) {}
    private final Map<ReplicaKey, PackageUserStateReplica> userReplicas = new HashMap<>();
    private record StateKey(String name, boolean crossUserSuspensions) {}
    private record PackageKey(String name, boolean factory, boolean crossUserSuspensions) {}
    private final Map<PackageKey, PackageStateReplica> stateReplicas = new HashMap<>();
    private final Map<SettingKey, PackageRuntimeState> runtimes = new HashMap<>();
    private final Map<String, SharedUserData> sharedUsers = new HashMap<>();
    private final Map<StateKey, SharedUserReplica> sharedReplicas = new HashMap<>();
    private java.util.List<String> sharedNames;
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
                    || !code.getPackageName().equals(name) || code.isFactory() != factory) {
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
        transientStates.clear();
        hiddenApiPolicies.clear();
        stateReplicas.clear();
        runtimes.clear();
        userReplicas.clear();
        sharedUsers.clear();
        sharedReplicas.clear();
        sharedNames = null;
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
        return com.android.server.pm.CapturedPackageSetting.withUsers(data,
                capturedUsers(name, factory), version, factory, crossUserSuspensions);
    }

    private java.util.List<PackageUserStateData> capturedUsers(String name, boolean factory)
            throws RemoteException, IOException {
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
        return java.util.List.copyOf(inputs);
    }

    /** Collected active code, saved owners and finalized dependency metadata. */
    public synchronized com.android.server.pm.PackageSetting newScannedSetting(String name,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        var factory = scannedSettingFactory(name, crossUserSuspensions);
        return factory == null ? null : factory.factory().get();
    }

    private record ScannedOwners(java.util.function.Supplier<com.android.server.pm.PackageSetting> factory,
            java.util.List<PackageUserStateData> users) {}

    private ScannedOwners scannedSettingFactory(
            String name, boolean crossUserSuspensions) throws RemoteException, IOException {
        PackageSettingData data = getSetting(name, false);
        if (data == null) return null;
        var userInputs = capturedUsers(name, false);
        PackageCode code = getCode(name, false);
        PackageSigningState saved = getSigningState(name, false);
        PackageUsageState times = getUsage(name);
        PackageSeInfoState labels = getSeInfo(name);
        PackageLibraryState dependencies = getLibraries(name);
        PackageTransientState transientState = getTransientState(name, false);
        if (code == null || saved == null || times == null || labels == null || dependencies == null || transientState == null) {
            throw new IOException("missing collected package owner");
        }
        long capturedVersion = version;
        java.util.function.Supplier<com.android.server.pm.PackageSetting> factory = () -> {
            var setting = com.android.server.pm.CapturedPackageSetting.withUsers(data, userInputs,
                    capturedVersion, false, crossUserSuspensions);
            PackageObjects.restoreCollectedCode(setting, code, capturedVersion, false);
            PackageObjects.restoreSavedSigning(setting, saved, capturedVersion, false);
            PackageObjects.restoreUsage(setting, times, capturedVersion);
            PackageObjects.restoreSeInfo(setting, labels, capturedVersion);
            PackageObjects.restoreLibraries(setting, dependencies, capturedVersion);
            PackageObjects.restoreTransientState(setting, transientState, capturedVersion, false);
            return setting;
        };
        return new ScannedOwners(factory, userInputs);
    }

    public synchronized PackageStateReplica getPackageStateReplica(String name, boolean crossUserSuspensions)
            throws RemoteException, IOException {
        return getPackageStateReplica(name, false, crossUserSuspensions);
    }

    public synchronized com.android.server.pm.PackageSetting newCapturedSetting(String name, boolean factory,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        var owners = capturedSettingFactory(name, factory, crossUserSuspensions);
        return owners == null ? null : owners.factory().get();
    }

    private ScannedOwners capturedSettingFactory(String name, boolean factoryScope, boolean crossUserSuspensions)
            throws RemoteException, IOException {
        PackageSettingData data = getSetting(name, factoryScope);
        if (data == null) return null;
        var userInputs = capturedUsers(name, factoryScope);
        PackageCode code = getCode(name, factoryScope);
        PackageSigningState saved = getSigningState(name, factoryScope);
        PackageRuntimeState runtime = getRuntimeState(name, factoryScope);
        PackageTransientState transientState = getTransientState(name, factoryScope);
        if (saved == null || runtime == null || transientState == null || runtime.hasCode() != (code != null)) {
            throw new IOException("missing captured package owner");
        }
        long capturedVersion = version;
        java.util.function.Supplier<com.android.server.pm.PackageSetting> factory = () -> {
            var setting = com.android.server.pm.CapturedPackageSetting.withUsers(data, userInputs,
                    capturedVersion, factoryScope, crossUserSuspensions);
            if (code != null) PackageObjects.restoreCollectedCode(setting, code, capturedVersion, factoryScope);
            PackageObjects.restoreSavedSigning(setting, saved, capturedVersion, factoryScope);
            PackageObjects.restoreRuntime(setting, runtime, capturedVersion, factoryScope);
            PackageObjects.restoreTransientState(setting, transientState, capturedVersion, factoryScope);
            return setting;
        };
        return new ScannedOwners(factory, userInputs);
    }

    public synchronized PackageStateReplica getPackageStateReplica(String name, boolean factoryScope,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        PackageKey key = new PackageKey(Objects.requireNonNull(name), factoryScope, crossUserSuspensions);
        if (stateReplicas.containsKey(key)) return stateReplicas.get(key);
        var owners = capturedSettingFactory(name, factoryScope, crossUserSuspensions);
        if (owners == null) { stateReplicas.put(key, null); return null; }
        var replicas = new java.util.LinkedHashMap<Integer, PackageUserStateReplica>();
        for (var user : owners.users()) {
            replicas.put(user.getUserId(), getUserStateReplica(name, factoryScope, user.getUserId(), crossUserSuspensions));
        }
        var replica = new PackageStateReplica(owners.factory(), replicas, getHiddenApiEnforcementPolicy(name, factoryScope));
        stateReplicas.put(key, replica);
        return replica;
    }

    public synchronized PackageRuntimeState getRuntimeState(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        SettingKey key = new SettingKey(Objects.requireNonNull(name), factory);
        if (runtimes.containsKey(key)) return runtimes.get(key);
        int length = endpoint.getRuntimeStateLength(name, factory);
        if (length == -1) { runtimes.put(key, null); return null; }
        if (length <= 0 || (length & 3) != 0) throw new IOException("invalid package runtime length: " + length);
        byte[] bytes = new byte[length];
        for (int offset = 0; offset < length;) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getRuntimeStateChunk(name, factory, offset, requested);
            if (chunk == null || chunk.length != requested) throw new IOException("incomplete package runtime chunk");
            System.arraycopy(chunk, 0, bytes, offset, requested); offset += requested;
        }
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            PackageRuntimeState state = PackageRuntimeState.read(in);
            if (in.dataAvail() != 0 || state.getVersion() != version || !state.getPackageName().equals(name)
                    || state.isFactory() != factory) throw new IOException("package runtime capture mismatch");
            runtimes.put(key, state);
            return state;
        } finally { in.recycle(); }
    }

    /** Assemble every setting and shared UID from this one capture before publication. */
    public synchronized PackageSnapshots.Data captureData(PackageSnapshots.Owner owner,
            boolean crossUserSuspensions) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(owner);
        var packages = capturePackageMap(false, crossUserSuspensions);
        var disabled = capturePackageMap(true, crossUserSuspensions);
        var shared = new java.util.LinkedHashMap<String, com.android.server.pm.pkg.SharedUserApi>();
        for (String name : getSharedUserNames()) {
            var replica = getSharedUserReplica(name, crossUserSuspensions);
            if (replica == null) throw new IOException("missing shared UID inventory owner");
            for (var member : replica.getPackageStates()) {
                if (packages.get(member.getPackageName()) != member) throw new IOException("shared UID member outside package inventory");
            }
            shared.put(name, replica);
        }
        var sharedIds = new java.util.HashSet<Integer>();
        for (var group : shared.values()) {
            if (!sharedIds.add(group.getAppId())) throw new IOException("duplicate shared UID identity");
        }
        validateSharedGroups(packages, shared, false);
        validateSharedGroups(disabled, shared, true);
        return new PackageSnapshots.Data(version, packages, disabled, shared, owner);
    }

    private void validateSharedGroups(java.util.Map<String, com.android.server.pm.pkg.PackageState> packages,
            java.util.Map<String, com.android.server.pm.pkg.SharedUserApi> shared, boolean factory)
            throws RemoteException, IOException {
        for (var state : packages.values()) {
            if (!state.hasSharedUser()) continue;
            var signing = getSigningState(state.getPackageName(), factory);
            var group = shared.get(signing.getSharedGroupName());
            if (group == null || group.getAppId() != state.getSharedUserAppId()) {
                throw new IOException("package shared UID outside group inventory");
            }
        }
    }

    private java.util.Map<String, com.android.server.pm.pkg.PackageState> capturePackageMap(
            boolean factory, boolean crossUserSuspensions) throws RemoteException, IOException {
        String[] names = endpoint.getPackageNames(factory);
        if (names == null) throw new IOException("missing package setting inventory");
        var result = new java.util.LinkedHashMap<String, com.android.server.pm.pkg.PackageState>();
        for (String name : names) {
            if (name == null || result.containsKey(name)) throw new IOException("invalid package setting inventory");
            var replica = getPackageStateReplica(name, factory, crossUserSuspensions);
            if (replica == null) throw new IOException("missing package setting inventory owner");
            result.put(name, replica);
        }
        return result;
    }

    public synchronized java.util.List<String> getSharedUserNames() throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        if (sharedNames != null) return sharedNames;
        String[] names = endpoint.getSharedUserNames();
        if (names == null) throw new IOException("missing shared UID inventory");
        var unique = new java.util.HashSet<String>();
        for (String name : names) {
            if (name == null || !unique.add(name)) throw new IOException("invalid shared UID inventory");
        }
        sharedNames = java.util.List.of(names);
        return sharedNames;
    }

    public synchronized SharedUserData getSharedUserData(String name) throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        Objects.requireNonNull(name);
        if (sharedUsers.containsKey(name)) return sharedUsers.get(name);
        int length = endpoint.getSharedUserStateLength(name);
        if (length == -1) { sharedUsers.put(name, null); return null; }
        if (length <= 0 || (length & 3) != 0) throw new IOException("invalid shared UID length");
        byte[] bytes = new byte[length];
        for (int offset = 0; offset < length;) {
            int requested = Math.min(CHUNK, length - offset);
            byte[] chunk = endpoint.getSharedUserStateChunk(name, offset, requested);
            if (chunk == null || chunk.length != requested) throw new IOException("incomplete shared UID chunk");
            System.arraycopy(chunk, 0, bytes, offset, requested);
            offset += requested;
        }
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            SharedUserData state = SharedUserData.CREATOR.createFromParcel(in);
            if (in.dataAvail() != 0 || state.getVersion() != version || !state.getName().equals(name)) {
                throw new IOException("shared UID capture mismatch");
            }
            sharedUsers.put(name, state);
            return state;
        } finally { in.recycle(); }
    }

    public synchronized SharedUserReplica getSharedUserReplica(String name, boolean crossUserSuspensions)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        StateKey key = new StateKey(Objects.requireNonNull(name), crossUserSuspensions);
        if (sharedReplicas.containsKey(key)) return sharedReplicas.get(key);
        SharedUserData state = getSharedUserData(name);
        if (state == null) { sharedReplicas.put(key, null); return null; }
        var members = new java.util.ArrayList<PackageStateReplica>();
        for (var member : state.getMembers()) {
            var replica = member.retained() == null
                ? getPackageStateReplica(member.name(), crossUserSuspensions)
                : member.retained().newReplica(crossUserSuspensions);
            if (replica == null) throw new IOException("missing shared UID package owner");
            members.add(replica);
        }
        var replica = new SharedUserReplica(state, members);
        sharedReplicas.put(key, replica);
        return replica;
    }

    public synchronized int getHiddenApiEnforcementPolicy(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        SettingKey key = new SettingKey(Objects.requireNonNull(name), factory);
        Integer cached = hiddenApiPolicies.get(key);
        if (cached != null) return cached;
        int policy = endpoint.getHiddenApiEnforcementPolicy(name, factory);
        if (policy != 0 && policy != 2) throw new IOException("invalid captured hidden API policy");
        hiddenApiPolicies.put(key, policy);
        return policy;
    }

    public synchronized PackageTransientState getTransientState(String name, boolean factory)
            throws RemoteException, IOException {
        if (closed) throw new IllegalStateException("package scan lease is closed");
        SettingKey key = new SettingKey(Objects.requireNonNull(name), factory);
        if (transientStates.containsKey(key)) return transientStates.get(key);
        byte[] bytes = endpoint.getTransientState(name, factory);
        if (bytes == null) { transientStates.put(key, null); return null; }
        if ((bytes.length & 3) != 0) throw new IOException("unaligned transient setting");
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var state = PackageTransientState.CREATOR.createFromParcel(in);
            if (in.dataAvail() != 0 || state.getVersion() != version
                    || !state.getPackageName().equals(name) || state.isFactory() != factory) {
                throw new IOException("package transient capture mismatch");
            }
            transientStates.put(key, state);
            return state;
        } finally { in.recycle(); }
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
