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

        public boolean crossUserSuspensions() { return crossUserSuspensions; }

        private final ThreadLocal<Boolean> refreshing = ThreadLocal.withInitial(() -> false);

        public long refresh() throws RemoteException, IOException {
            long retainedVersion;
            synchronized (this) {
                if (closed) throw new IllegalStateException("package replica store is closed");
                if (refreshing.get()) {
                    if (current == null) throw new IllegalStateException("initial package capture is in progress");
                    return current.version;
                }
                retainedVersion = current == null ? 0 : current.version;
                refreshing.set(true);
            }
            Data next = null;
            Data previous = null;
            try {
                var endpoint = source.capture();
                if (endpoint == null) throw new IOException("missing native snapshot endpoint");
                try (var lease = new PackageScanLease(endpoint)) {
                    long version = lease.getVersion();
                    synchronized (this) {
                        if (closed) throw new IllegalStateException("package replica store is closed");
                        if (current != null) {
                            if (version < retainedVersion) throw new IOException("native snapshot version moved backwards");
                            if (version <= current.version) return current.version;
                        }
                    }
                    next = lease.captureData(owner, crossUserSuspensions);
                }
                synchronized (this) {
                    if (closed) throw new IllegalStateException("package replica store is closed");
                    // An independent capture may have published while this RPC
                    // was assembling its immutable graph. Retire the loser.
                    if (current != null && next.version <= current.version) return current.version;
                    previous = current;
                    current = next;
                    next = null;
                    return current.version;
                }
            } finally {
                refreshing.remove();
                if (next != null) next.close();
                if (previous != null) previous.close();
            }
        }

        private Data capture() {
            long retainedVersion;
            synchronized (this) {
                if (closed) throw new IllegalStateException("package replica store is closed");
                var candidate = NativeInstallPermissionScope.candidate(this);
                if (candidate != null) { candidate.retain(); return candidate; }
                if (current == null) throw new IllegalStateException("native package replica is unavailable");
                retainedVersion = current.version;
                // Same-thread recursion alone may use the retained immutable
                // graph while its next capture is assembled outside locks.
                if (versions == null || refreshing.get()) { current.retain(); return current; }
            }
            long observed = versions.currentVersion();
            synchronized (this) {
                if (closed || current == null) throw new IllegalStateException("package replica store is closed");
                if (observed < retainedVersion) throw new IllegalStateException("native package version moved backwards");
                if (observed <= current.version) { current.retain(); return current; }
            }
            try { refresh(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            catch (IOException failure) { throw new IllegalStateException("native package replica refresh failed", failure); }
            synchronized (this) {
                if (closed || current == null) throw new IllegalStateException("package replica store is closed");
                current.retain();
                return current;
            }
        }

        /** Resolution consumes the native query capture, without materializing Java package owners. */
        public android.content.pm.ResolveInfo resolveServiceQuery(android.content.Intent intent, String type,
                long flags, int user, int filterUid, int filterPid, boolean start) {
            Data candidate;
            synchronized (this) {
                if (closed) throw new IllegalStateException("package replica store is closed");
                candidate = NativeInstallPermissionScope.candidate(this);
                if (candidate != null) candidate.retain();
            }
            if (candidate != null) {
                try (var computer = new ComputerSnapshot(candidate)) {
                    return computer.resolveServiceInternal(intent, type, flags, user, filterUid, filterPid, start);
                } finally { candidate.release(); }
            }
            int callingUid = android.os.Binder.getCallingUid();
            int callingPid = android.os.Binder.getCallingPid();
            try {
                var endpoint = source.capture();
                if (endpoint == null) throw new IOException("missing native snapshot endpoint");
                try (var lease = new PackageScanLease(endpoint)) {
                    var computer = lease.captureComputer();
                    try {
                        synchronized (this) {
                            if (closed) throw new IllegalStateException("package replica store is closed");
                        }
                        return ComputerSnapshot.resolveRecord(computer.resolveServiceInternalRecord(intent, type,
                                flags, user, filterUid, filterPid, start, callingUid, callingPid));
                    } finally { computer.close(); }
                }
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            catch (IOException failure) { throw new IllegalStateException("native service query capture failed", failure); }
        }

        public long getVersion() { var data = capture(); try { return data.version; } finally { data.release(); } }
        public ComputerSnapshot computer() {
            var data = capture(); try { return new ComputerSnapshot(data); } finally { data.release(); }
        }
        @Override public void close() {
            Data previous;
            synchronized (this) { if (closed) return; closed = true; previous = current; current = null; }
            try { if (previous != null) previous.close(); }
            finally { if (versions != null) versions.close(); }
        }
        public PackageManagerLocal.UnfilteredSnapshot unfiltered() {
            var data = capture(); try { return PackageSnapshots.unfiltered(data); } finally { data.release(); }
        }
        public PackageManagerLocal.FilteredSnapshot filtered(int callingUid, UserHandle user, PackageState uncommitted) {
            var data = capture(); try { return PackageSnapshots.filtered(data, callingUid, user, uncommitted); } finally { data.release(); }
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
        private final Map<Integer, Object> uidOwners;
        private record NameKey(String name, int callingUid) {}
        private record CallerKey(int uid, int pid) {}
        private final Map<CallerKey, android.content.pm.IPackageManager> readQueries = new java.util.HashMap<>();
        private com.android.server.utils.WatchedArrayMap<String, com.android.server.utils.WatchedLongSparseArray<android.content.pm.SharedLibraryInfo>> libraryRegistry;
        private com.android.server.utils.WatchedArrayMap<String, Integer> frozenPackages;
        private android.content.pm.ResolveInfo instantInstallerInfo;
        private boolean instantInstallerInfoRead;
        private android.content.pm.SigningDetails platformSigning;
        private final Map<NameKey, String> resolvedNames = new java.util.HashMap<>();
        private int references = 1;
        private boolean ownerClosed;
        private Data parent;

        Data withoutPackages(java.util.Set<String> names) {
            var active = new LinkedHashMap<>(packages);
            names.forEach(active::remove);
            retain();
            var result = new Data(version, active, disabled, sharedUsers, owner, computer, uidOwners);
            result.parent = this;
            return result;
        }

        public Data(long version, Map<String, PackageState> packages,
                Map<String, PackageState> disabled, Map<String, SharedUserApi> sharedUsers,
                Owner owner) {
            this(version, packages, disabled, sharedUsers, owner, null, null);
        }

        Data(long version, Map<String, PackageState> packages,
                Map<String, PackageState> disabled, Map<String, SharedUserApi> sharedUsers,
                Owner owner, IPackageComputer computer, Map<Integer, Object> uidOwners) {
            this.version = version;
            this.computer = computer;
            this.uidOwners = uidOwners;
            this.packages = copy(packages);
            this.disabled = copy(disabled);
            this.sharedUsers = copy(sharedUsers);
            this.owner = Objects.requireNonNull(owner);
        }

        android.content.pm.IPackageManager readQueries(int uid, int pid) throws RemoteException {
            if (computer == null) throw new IllegalStateException("native package query capture unavailable");
            var key = new CallerKey(uid, pid);
            synchronized (this) {
                var query = readQueries.get(key);
                if (query != null) return query;
                retain();
            }
            try {
                var binder = computer.getPackageManagerQueryBinder(uid, pid);
                if (binder == null) throw new IllegalStateException("missing retained read-only query binder");
                var query = android.content.pm.IPackageManager.Stub.asInterface(android.os.Binder.allowBlocking(binder));
                synchronized (this) { return readQueries.computeIfAbsent(key, ignored -> query); }
            } finally { release(); }
        }

        String resolveName(String name, int callingUid) throws RemoteException {
            if (computer == null) throw new IllegalStateException("native package query capture unavailable");
            var key = new NameKey(name, callingUid);
            synchronized (this) {
                if (resolvedNames.containsKey(key)) return resolvedNames.get(key);
                retain();
            }
            try {
                String resolved = computer.resolveInternalPackageName(name,
                        android.content.pm.PackageManager.VERSION_CODE_HIGHEST, callingUid);
                synchronized (this) {
                    if (!resolvedNames.containsKey(key)) resolvedNames.put(key, resolved);
                    return resolvedNames.get(key);
                }
            } finally { release(); }
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
            Data retainedParent;
            synchronized (this) {
                if (references == 0) throw new IllegalStateException("package query capture already released");
                endpoint = --references == 0 ? computer : null;
                retainedParent = references == 0 ? parent : null;
            }
            if (retainedParent != null) { retainedParent.release(); return; }
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
        protected final Data data;
        private volatile boolean closed;

        Scope(Data data) { data.retain(); this.data = data; }

        protected void checkClosed() {
            if (closed) throw new IllegalStateException("Snapshot already closed");
        }

        @Override
        public void close() {
            synchronized (this) { if (closed) return; closed = true; }
            data.release();
        }
    }

    /** Typed internal query adapter retaining the same immutable graph as replica lookups. */
    public static final class ComputerSnapshot extends Scope {
        ComputerSnapshot(Data data) { super(data); }

        private synchronized IPackageComputer endpoint() {
            checkClosed();
            if (data.computer == null) throw new IllegalStateException("native package query capture unavailable");
            return data.computer;
        }

        public Integer crossProfileDomainApproval(android.content.Intent intent,String type,
                long flags,int source,int parent) {
            int[] value;try{value=endpoint().getCrossProfileDomainApproval(intent,type,flags,source,parent);}
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
            if(value==null)return null;if(value.length!=1)throw new IllegalStateException("cross-profile approval record differs");return value[0];
        }

        public int[] selectPreferredActivity(android.content.Intent intent,String type,long flags,
                android.content.ComponentName[] candidates,int[] matches,boolean always,boolean remove,
                boolean filtered,boolean provisioned,int user) {
            try{return endpoint().selectPreferredActivity(intent,type,flags,candidates,matches,always,remove,
                    filtered,provisioned,user,android.os.Binder.getCallingUid(),android.os.Binder.getCallingPid());}
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
        }

        public java.util.List<android.content.pm.ProviderInfo> queryRawProviders(String process,
                String metadata,int uid,long flags,int user) {
            byte[] record;try{record=endpoint().queryRawProvidersRecord(process,metadata,uid,flags,user);}
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
            var parcel=rawParcel(record);try{var values=parcel.createTypedArrayList(android.content.pm.ProviderInfo.CREATOR);if(parcel.dataAvail()!=0)throw new IllegalStateException("raw providers trailing data");return values;}finally{parcel.recycle();}
        }
        public void queryRawSyncProviders(java.util.List<String> names,
                java.util.List<android.content.pm.ProviderInfo> providers,boolean safe,int user) {
            byte[] record;try{record=endpoint().queryRawSyncProvidersRecord(safe,user);}
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
            var parcel=rawParcel(record);try{int count=parcel.readInt();if(count<0||count>parcel.dataAvail()/4)throw new IllegalStateException("raw sync providers count");for(int i=0;i<count;i++){names.add(parcel.readString());providers.add(parcel.readTypedObject(android.content.pm.ProviderInfo.CREATOR));}if(parcel.dataAvail()!=0)throw new IllegalStateException("raw sync providers trailing data");}finally{parcel.recycle();}
        }
        public void dumpRawComponents(int kind,java.io.PrintWriter writer,
                com.android.server.pm.DumpState state,String name) {
            byte[] record;try{record=endpoint().dumpRawComponentsRecord(kind,name,com.android.server.pm.NativeDumpStateRecord.capture(state));}
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
            var parcel=rawParcel(record);try{boolean title=parcel.readBoolean();String text=parcel.readString();if(parcel.dataAvail()!=0)throw new IllegalStateException("raw component dump trailing data");writer.print(text);state.setTitlePrinted(title);}finally{parcel.recycle();}
        }
        private static android.os.Parcel rawParcel(byte[] record){
            if(record==null)throw new IllegalStateException("raw registry capture unavailable");
            var parcel=android.os.Parcel.obtain();parcel.unmarshall(record,0,record.length);parcel.setDataPosition(0);return parcel;
        }

        public java.util.List<android.content.pm.ResolveInfo> queryRawComponents(int kind,
                android.content.Intent intent,String type,long flags,android.content.ComponentName[] subset,int user) {
            byte[] record;
            try { record=endpoint().queryRawComponentsRecord(kind,intent,type,flags,intent.getPackage(),subset,user,
                    android.os.Binder.getCallingUid(),android.os.Binder.getCallingPid()); }
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
            if(record==null)throw new IllegalStateException("raw component capture unavailable");
            var parcel=android.os.Parcel.obtain();
            try{parcel.unmarshall(record,0,record.length);parcel.setDataPosition(0);var result=parcel.createTypedArrayList(android.content.pm.ResolveInfo.CREATOR);if(parcel.dataAvail()!=0)throw new IllegalStateException("raw components trailing bytes");return result;}
            finally{parcel.recycle();}
        }
        public android.content.pm.ProviderInfo queryRawProvider(String authority,long flags,int user) {
            try{return endpoint().queryRawProvider(authority,flags,user);}
            catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
        }

        public void querySyncProviders(boolean safeMode, java.util.List<String> names,
                java.util.List<android.content.pm.ProviderInfo> providers) {
            byte[] record;
            try { record = endpoint().getSyncProvidersRecord(safeMode, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            if (record == null) throw new IllegalStateException("sync provider owner unavailable");
            var parcel = android.os.Parcel.obtain();
            try {
                parcel.unmarshall(record,0,record.length);parcel.setDataPosition(0);
                int count = parcel.readInt();
                if (count < 0 || count > parcel.dataAvail()/4) throw new IllegalStateException("sync provider record count");
                var addedNames = new java.util.ArrayList<String>(count);
                var addedProviders = new java.util.ArrayList<android.content.pm.ProviderInfo>(count);
                for (int i=0;i<count;i++) { addedNames.add(parcel.readString()); addedProviders.add(parcel.readTypedObject(android.content.pm.ProviderInfo.CREATOR)); }
                if (parcel.dataAvail()!=0) throw new IllegalStateException("sync provider record trailing bytes");
                if (!addedNames.isEmpty()) names.addAll(addedNames);
                if (!addedProviders.isEmpty()) providers.addAll(addedProviders);
            } finally { parcel.recycle(); }
        }

        public byte[] getLegacyPermissionDefinitionsRecord() {
            checkClosed();
            try { return endpoint().getLegacyPermissionDefinitionsRecord(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public long getVersion() { checkClosed(); return data.version; }

        public android.content.pm.ActivityInfo getNativeResolverActivity() {
            try {return endpoint().getNativeResolverActivity();}catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
        }
        public boolean isNativeResolverReplaced() {
            try {return endpoint().isNativeResolverReplaced();}catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
        }
        public android.content.pm.IPackageManager readQueries(int uid,int pid) {
            Bridge.enforceSystemUid();
            checkClosed();
            try { return data.readQueries(uid,pid); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        public android.content.pm.IPackageManager readQueries() {
            checkClosed();
            try { return data.readQueries(android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        private String resolvePackageName(String name, int callingUid) {
            checkClosed();
            try { return data.resolveName(name, callingUid); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public PackageStateInternal getPackageStateInternal(String name) {
            return getPackageStateInternal(name, android.os.Binder.getCallingUid());
        }

        public PackageStateInternal getPackageStateInternal(String name, int callingUid) {
            String resolved = resolvePackageName(name, callingUid);
            return (PackageStateInternal) data.packages.get(resolved);
        }

        public PackageStateInternal getPackageStateFiltered(String name, int callingUid, int userId) {
            String resolved;
            try { resolved = endpoint().getPackageStateFilteredName(name, callingUid, userId); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            return resolved == null ? null : (PackageStateInternal) data.packages.get(resolved);
        }

        public int getUidTargetSdkVersion(int uid) {
            try { return endpoint().getUidTargetSdkVersion(uid); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public PackageStateInternal getDisabledSystemPackage(String name) {
            checkClosed();
            return (PackageStateInternal) data.disabled.get(name);
        }

        public int getPackageUidInternal(String name, long flags, int userId) {
            try { return endpoint().getPackageUidInternal(name, flags, userId); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isPackageEphemeral(int userId, String name) {
            PackageStateInternal state = getPackageStateInternal(name);
            return state != null && state.getUserStateOrDefault(userId).isInstantApp();
        }

        public int getPackageTargetSdkVersion(String name) {
            PackageStateInternal state = getPackageStateInternal(name);
            return state != null && state.getPkg() != null ? state.getPkg().getTargetSdkVersion()
                    : android.os.Build.VERSION_CODES.CUR_DEVELOPMENT;
        }

        public com.android.server.pm.pkg.AndroidPackage getPackage(String name) {
            String resolved = resolvePackageName(name, android.os.Binder.getCallingUid());
            PackageState state = data.packages.get(resolved);
            return state == null ? null : state.getAndroidPackage();
        }

        public com.android.server.pm.pkg.AndroidPackage getAndroidPackage(String name) {
            return getPackage(name);
        }

        public android.util.ArrayMap<String, PackageStateInternal> getPackageStates() {
            checkClosed();
            var result = new android.util.ArrayMap<String, PackageStateInternal>();
            data.packages.forEach((name, state) -> result.put(name, (PackageStateInternal) state));
            return result;
        }

        public void forEachPackageState(java.util.function.Consumer<PackageStateInternal> action) {
            for (var state : getPackageStates().values()) action.accept(state);
        }

        public void forEachPackage(java.util.function.Consumer<com.android.server.pm.pkg.AndroidPackage> action) {
            for (var state : getPackageStates().values()) {
                if (state.getPkg() != null) action.accept(state.getPkg());
            }
        }

        private Object uidOwner(int appId) {
            checkClosed();
            if (data.uidOwners == null) throw new IllegalStateException("registered UID owner capture unavailable");
            return data.uidOwners.get(appId);
        }

        public SharedUserApi getSharedUserApi(int appId) {
            Object owner = uidOwner(appId);
            return owner instanceof SharedUserApi group ? group : null;
        }

        public android.util.ArraySet<PackageStateInternal> getSharedUserPackages(int appId) {
            var result = new android.util.ArraySet<PackageStateInternal>();
            SharedUserApi group = getSharedUserApi(appId);
            if (group != null) for (var state : group.getPackageStates()) result.add((PackageStateInternal) state);
            return result;
        }

        public java.util.List<com.android.server.pm.pkg.AndroidPackage> getPackagesForAppId(int appId) {
            Object owner = uidOwner(appId);
            var result = new java.util.ArrayList<com.android.server.pm.pkg.AndroidPackage>();
            if (owner instanceof SharedUserApi group) {
                for (var state : group.getPackageStates()) if (state.getAndroidPackage() != null) result.add(state.getAndroidPackage());
            } else if (owner instanceof PackageStateInternal state && state.getPkg() != null) {
                result.add(state.getPkg());
            }
            return result;
        }

        public SharedUserApi getSharedUser(int appId) { return getSharedUserApi(appId); }

        public android.util.ArrayMap<String, SharedUserApi> getSharedUsers() {
            checkClosed();
            var result = new android.util.ArrayMap<String, SharedUserApi>();
            result.putAll(data.sharedUsers);
            return result;
        }

        public String[] getAllAvailablePackageNames() {
            checkClosed();
            var names = new java.util.ArrayList<String>();
            data.packages.forEach((name, state) -> { if (state.getAndroidPackage() != null) names.add(name); });
            return names.toArray(new String[0]);
        }

        public String[] getSharedUserPackagesForPackage(String name, int userId) {
            checkClosed();
            PackageState state = data.packages.get(name);
            SharedUserApi group = state == null || !state.hasSharedUser() ? null
                    : getSharedUserApi(state.getSharedUserAppId());
            var names = new java.util.ArrayList<String>();
            if (group != null) for (var member : group.getPackageStates()) {
                if (member.getUserStateOrDefault(userId).isInstalled()) names.add(member.getPackageName());
            }
            return names.toArray(new String[0]);
        }

        public String[] filterOnlySystemPackages(String[] names) {
            checkClosed();
            var result = new java.util.ArrayList<String>();
            if (names != null) for (String name : names) {
                if (name == null) continue;
                var state = getPackageStateInternal(name);
                if (state != null && state.getAndroidPackage() != null && state.isSystem()) result.add(name);
            }
            return result.toArray(new String[0]);
        }

        public android.util.Pair<PackageStateInternal, SharedUserApi> getPackageOrSharedUser(int appId) {
            Object owner = uidOwner(appId);
            if (owner instanceof SharedUserApi group) return new android.util.Pair<>(null, group);
            if (owner instanceof PackageStateInternal state) return new android.util.Pair<>(state, null);
            return null;
        }

        public java.util.List<PackageStateInternal> getVolumePackages(String volumeUuid) {
            checkClosed();
            var result = new java.util.ArrayList<PackageStateInternal>();
            data.packages.forEach((name, state) -> {
                if (java.util.Objects.equals(volumeUuid, state.getVolumeUuid())) result.add((PackageStateInternal) state);
            });
            return result;
        }

        public android.util.ArrayMap<String, PackageStateInternal> getDisabledSystemPackageStates() {
            checkClosed();
            var result = new android.util.ArrayMap<String, PackageStateInternal>();
            data.disabled.forEach((name, state) -> result.put(name, (PackageStateInternal) state));
            return result;
        }

        public android.util.ArraySet<String> getEnabledComponents(String name, int userId) {
            var state = getPackageStateInternal(name);
            return state == null ? new android.util.ArraySet<>() : state.getUserStateOrDefault(userId).getEnabledComponents();
        }

        public android.util.ArraySet<String> getDisabledComponents(String name, int userId) {
            var state = getPackageStateInternal(name);
            return state == null ? new android.util.ArraySet<>() : state.getUserStateOrDefault(userId).getDisabledComponents();
        }

        public int getApplicationEnabledState(String name, int userId) {
            var state = getPackageStateInternal(name);
            return state == null ? 0 : state.getUserStateOrDefault(userId).getEnabledState();
        }

        public long getCeDataInode(String name, int userId) {
            PackageStateInternal state = getPackageStateInternal(name);
            return state == null ? 0 : state.getUserStateOrDefault(userId).getCeDataInode();
        }

        public boolean wasPackageEverLaunched(String name, int userId) {
            PackageStateInternal state = getPackageStateInternal(name);
            if (state == null) throw new IllegalArgumentException("Unknown package: " + name);
            return !state.getUserStateOrDefault(userId).isNotLaunched();
        }

        public boolean isPackagePersistent(String name) {
            PackageStateInternal state = getPackageStateInternal(name);
            return state != null && state.getPkg() != null && state.isSystem() && state.getPkg().isPersistent();
        }

        public android.content.pm.SigningDetails getSigningDetails(String name) {
            checkClosed();
            PackageState state = data.packages.get(name);
            var pkg = state == null ? null : state.getAndroidPackage();
            return pkg == null ? null : pkg.getSigningDetails();
        }

        public android.content.pm.SigningDetails getSigningDetails(int uid) {
            Object owner = uidOwner(android.os.UserHandle.getAppId(uid));
            if (owner instanceof SharedUserApi group) return group.getSigningDetails();
            if (owner instanceof PackageStateInternal state) return state.getSigningDetails();
            return android.content.pm.SigningDetails.UNKNOWN;
        }

        public byte[] getDiagnosticRecord(int kind, int type, String name, String[] permissions, boolean checkIn, byte[] state) {
            try {
                byte[] record = endpoint().getDiagnosticRecord(kind, type, name, permissions, checkIn, state);
                if (record == null) throw new IllegalStateException("native diagnostic record owner unavailable");
                return record;
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.os.IBinder[] getPreferredRecordTokens(int user, int kind) {
            try { return endpoint().getPreferredRecordTokens(user, kind); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean hasInstantApplicationMetadata(String name, int user) {
            try { return endpoint().hasInstantApplicationMetadata(name, user); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isPermissionUpgradeNeeded(int user) {
            try { return endpoint().isPermissionUpgradeNeeded(user); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ResolveInfo resolveIntentInternal(android.content.Intent intent, String type,
                long flags, long privateFlags, int user, boolean start, int filterUid, int filterPid) {
            try { return resolveRecord(endpoint().resolveIntentInternalRecord(intent, type, flags, privateFlags, user, start,
                    filterUid, filterPid, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid())); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        public android.content.pm.ResolveInfo resolveServiceInternal(android.content.Intent intent, String type,
                long flags, int user, int filterUid, int filterPid, boolean start) {
            try { return resolveRecord(endpoint().resolveServiceInternalRecord(intent, type, flags, user, filterUid, filterPid,
                    start, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid())); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        public java.util.List<android.content.pm.ResolveInfo> queryIntentReceiversInternal(android.content.Intent intent,
                String type, long flags, int filterUid, int filterPid, int user, boolean send) {
            try { return resolveList(endpoint().queryIntentReceiversInternalRecord(intent, type, flags, user, filterUid, filterPid,
                    send, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid())); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        private static android.content.pm.ResolveInfo resolveRecord(byte[] record) {
            if (record == null) return null;
            var parcel = android.os.Parcel.obtain();
            try {
                parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
                var result = android.content.pm.ResolveInfo.CREATOR.createFromParcel(parcel);
                if (result == null) throw new IllegalStateException("native resolution object capture differs");
                restoreAuxiliary(parcel,java.util.List.of(result));
                if(parcel.dataAvail()!=0)throw new IllegalStateException("native resolution object trailing bytes");
                return result;
            } finally { parcel.recycle(); }
        }

        public java.util.List<android.content.pm.ResolveInfo> queryIntentServicesInternal(android.content.Intent intent,
                String type, long flags, int user, int filterUid, int filterPid, boolean includeInstant, boolean forStart) {
            try { return resolveList(endpoint().queryIntentServicesInternalRecord(intent, type, flags, user, filterUid, filterPid,
                    includeInstant, forStart, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid())); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public java.util.List<android.content.pm.ResolveInfo> queryIntentActivitiesInternal(android.content.Intent intent,
                String resolvedType, long flags, long privateFlags, int filterUid, int filterPid, int user,
                boolean forStart, boolean dynamicSplits) {
            byte[] record;
            try { record = endpoint().queryIntentActivitiesInternalRecord(intent, resolvedType, flags, privateFlags,
                    filterUid, filterPid, user, forStart, dynamicSplits,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            return resolveList(record);
        }

        private static void restoreAuxiliary(android.os.Parcel parcel,java.util.List<android.content.pm.ResolveInfo> results) {
            if(parcel.dataAvail()==0)return;
            int count=parcel.readInt();if(count!=results.size())throw new IllegalStateException("native auxiliary record inventory differs");
            for(var result:results) {
                if(!parcel.readBoolean())continue;
                android.content.ComponentName failure=null;
                if(parcel.readBoolean())failure=new android.content.ComponentName(parcel.readString(),parcel.readString());
                String name=java.util.Objects.requireNonNull(parcel.readString());long version=parcel.readLong();
                String split=java.util.Objects.requireNonNull(parcel.readString());
                result.auxiliaryInfo=new android.content.pm.AuxiliaryResolveInfo(failure,name,version,split);
            }
        }
        private static java.util.List<android.content.pm.ResolveInfo> resolveList(byte[] record) {
            if (record == null) throw new IllegalStateException("native resolution list capture unavailable");
            var parcel = android.os.Parcel.obtain();
            try {
                parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
                var result = parcel.createTypedArrayList(android.content.pm.ResolveInfo.CREATOR);
                if (result == null) throw new IllegalStateException("native resolution list capture differs");
                restoreAuxiliary(parcel,result);
                if(parcel.dataAvail()!=0)throw new IllegalStateException("native resolution list trailing bytes");
                return result;
            } finally { parcel.recycle(); }
        }

        public boolean isResolveActivityComponent(android.content.pm.ComponentInfo component) {
            android.content.ComponentName resolver;
            try { resolver = endpoint().getResolverComponent(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            if (resolver == null) throw new IllegalStateException("boot resolver component owner unavailable");
            return resolver.getPackageName().equals(component.packageName) && resolver.getClassName().equals(component.name);
        }

        public java.util.List<String> getApksInApex(String name) {
            try { var result = endpoint().getApksInApex(name); return result == null ? null : new java.util.ArrayList<>(java.util.Arrays.asList(result)); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isUpgradingFromLowerThan(int sdk) {
            try { return endpoint().isUpgradingFromLowerThan(sdk); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        // SuspendPackageHelper readonly methods, pinned AOSP (Apache 2.0).
        public android.os.Bundle getSuspendedPackageLauncherExtras(String name, int user) {
            var state = getPackageStateInternal(name, android.os.Binder.getCallingUid());
            if (state == null) return null;
            var extras = new android.os.Bundle();
            var userState = state.getUserStateOrDefault(user);
            if (userState.isSuspended()) {
                var params = userState.getSuspendParams();
                for (int i = 0; i < params.size(); i++) {
                    var value = params.valueAt(i);
                    if (value != null && value.getLauncherExtras() != null) extras.putAll(value.getLauncherExtras());
                }
            }
            return extras.size() > 0 ? extras : null;
        }

        public boolean isPackageSuspended(String name, int user) {
            var state = getPackageStateInternal(name, android.os.Binder.getCallingUid());
            return state != null && state.getUserStateOrDefault(user).isSuspended();
        }

        public android.content.pm.UserPackage getSuspendingPackage(String name, int user) {
            var state = getPackageStateInternal(name, android.os.Binder.getCallingUid());
            if (state == null) return null;
            var userState = state.getUserStateOrDefault(user);
            if (!userState.isSuspended()) return null;
            android.content.pm.UserPackage last = null, system = null, quarantine = null;
            var params = userState.getSuspendParams();
            for (int i = 0; i < params.size(); i++) {
                last = params.keyAt(i);
                var value = params.valueAt(i);
                if ("android".equals(last.packageName)) system = last;
                if (value.isQuarantined() && quarantine == null) quarantine = last;
            }
            return quarantine != null ? quarantine : system != null ? system : last;
        }

        public android.content.pm.SuspendDialogInfo getSuspendedDialogInfo(String name,
                android.content.pm.UserPackage suspender, int user) {
            var state = getPackageStateInternal(name, android.os.Binder.getCallingUid());
            if (state == null) return null;
            var userState = state.getUserStateOrDefault(user);
            if (!userState.isSuspended()) return null;
            var params = userState.getSuspendParams();
            if (params == null) return null;
            var value = params.get(suspender);
            return value == null ? null : value.getDialogInfo();
        }

        public String[] getKnownPackageNames(int kind, int user) {
            try {
                var result = endpoint().getKnownPackageNames(kind, user,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
                if (result == null) throw new IllegalStateException("known package table returned null array");
                return result;
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.SigningDetails getPlatformSigningDetails() {
            checkClosed();
            synchronized (data) { if (data.platformSigning != null) return data.platformSigning; }
            {
                byte[] record;
                try { record = endpoint().getPlatformSigningDetailsRecord(); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
                if (record == null) throw new IllegalStateException("boot platform signing owner unavailable");
                var parcel = android.os.Parcel.obtain();
                try {
                    parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
                    var signing = android.content.pm.SigningDetails.CREATOR.createFromParcel(parcel);
                    if (signing == null || parcel.dataAvail() != 0) throw new IllegalStateException("boot platform signing capture differs");
                    synchronized (data) {
                        if (data.platformSigning == null) data.platformSigning = signing;
                        return data.platformSigning;
                    }
                } finally { parcel.recycle(); }
            }
        }

        public void enforceCrossUserOrProfilePermission(int uid, int user, boolean full, boolean shell, String message) {
            checkClosed();
            if (user < 0) throw new IllegalArgumentException("Invalid userId " + user);
            if (shell) com.android.server.pm.NativeUserManagerBridge.enforceShellRestriction(uid, user);
            try {
                if (endpoint().hasCrossUserPermission(uid, user, full,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid())) return;
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            boolean profile = com.android.server.pm.NativeUserManagerBridge.isSameProfileGroup(android.os.UserHandle.getUserId(uid), user);
            if (profile && com.android.server.pm.NativeUserManagerBridge.hasCrossProfilePermission(uid, getPackage(uid).getPackageName())) return;
            String denial = (message == null ? "" : message + ": ") + "UID " + uid
                    + " requires android.permission.INTERACT_ACROSS_USERS_FULL";
            if (!full) {
                denial += " or android.permission.INTERACT_ACROSS_USERS";
                if (profile) denial += " or android.permission.INTERACT_ACROSS_PROFILES";
            }
            denial += " to access user " + user + ".";
            android.util.Slog.w("PackageManager", denial);
            throw new SecurityException(denial);
        }

        public boolean activitySupportsIntentAsUser(android.content.ComponentName resolver,
                android.content.ComponentName component, android.content.Intent intent, String resolvedType, int user) {
            try { return endpoint().activitySupportsIntentAsUser(resolver, component, intent, resolvedType, user,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ResolveInfo getInstantAppInstallerInfo() {
            checkClosed();
            synchronized (data) { if (data.instantInstallerInfoRead) return data.instantInstallerInfo; }
            {
                android.content.pm.ResolveInfo info = null;
                byte[] record;
                try { record = endpoint().getInstantAppInstallerInfoRecord(); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
                if (record != null) {
                    var parcel = android.os.Parcel.obtain();
                    try {
                        parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
                        info = android.content.pm.ResolveInfo.CREATOR.createFromParcel(parcel);
                        if (parcel.dataAvail() != 0) throw new IllegalStateException("instant installer info capture has trailing data");
                    } finally { parcel.recycle(); }
                }
                synchronized (data) {
                    if (!data.instantInstallerInfoRead) {
                        data.instantInstallerInfo = info;
                        data.instantInstallerInfoRead = true;
                    }
                    return data.instantInstallerInfo;
                }
            }
        }

        public com.android.server.utils.WatchedArrayMap<String, Integer> getFrozenPackages() {
            checkClosed();
            synchronized (data) { if (data.frozenPackages != null) return data.frozenPackages; }
            {
                String[] names;
                int[] counts;
                try { names = endpoint().getFrozenPackageNames(); counts = endpoint().getFrozenPackageCounts(); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
                if (names == null || counts == null || names.length != counts.length)
                    throw new IllegalStateException("frozen package capture shape differs");
                var result = new com.android.server.utils.WatchedArrayMap<String, Integer>();
                for (int i = 0; i < names.length; i++) {
                    if (names[i] == null || counts[i] <= 0 || result.containsKey(names[i]))
                        throw new IllegalStateException("frozen package capture owner differs");
                    result.put(names[i], counts[i]);
                }
                synchronized (data) {
                    if (data.frozenPackages == null) data.frozenPackages = result.snapshot();
                    return data.frozenPackages;
                }
            }
        }

        public void checkPackageFrozen(String name) {
            if (!getFrozenPackages().containsKey(name)) android.util.Slog.wtf("PackageManager",
                    "Expected " + name + " to be frozen!", new Throwable());
        }

        public android.content.ComponentName getInstantAppInstallerComponent() {
            try { return endpoint().getInstantAppInstallerComponent(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ProviderInfo resolveContentProvider(String authority, long flags, int user, int filterUid) {
            try { return endpoint().resolveContentProvider(authority, flags, user, filterUid,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ProviderInfo getGrantImplicitAccessProviderInfo(int recipientUid, String authority) {
            int caller = android.os.Binder.getCallingUid();
            var contacts = resolveContentProvider("com.android.contacts", 0, android.os.UserHandle.getUserId(caller), caller);
            if (contacts == null || contacts.applicationInfo == null
                    || android.os.UserHandle.getAppId(contacts.applicationInfo.uid) != android.os.UserHandle.getAppId(caller))
                throw new SecurityException(caller + " is not allow to call grantImplicitAccess");
            long token = android.os.Binder.clearCallingIdentity();
            try { return resolveContentProvider(authority, 0, android.os.UserHandle.getUserId(recipientUid), caller); }
            finally { android.os.Binder.restoreCallingIdentity(token); }
        }

        public android.content.pm.ActivityInfo getActivityInfoCrossProfile(android.content.ComponentName component,
                long flags, int user) {
            try { return endpoint().getActivityInfoCrossProfile(component, flags, user,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ActivityInfo getActivityInfoInternal(android.content.ComponentName component,
                long flags, int filterUid, int user) {
            try { return endpoint().getActivityInfoInternal(component, flags, filterUid, user,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean canAccessComponent(int filterUid, android.content.ComponentName component, int user) {
            try { return endpoint().canAccessComponent(filterUid, component, user,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public int[] getVisibilityAllowList(String name, int user) {
            try { return endpoint().getVisibilityAllowList(name, user, false); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.util.SparseArray<int[]> getVisibilityAllowLists(String name, int[] users) {
            checkClosed();
            try { if (endpoint().getVisibilityAllowList(name, 0, true) == null) return null; }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            var result = new android.util.SparseArray<int[]>();
            for (int user : users) {
                int[] ids = getVisibilityAllowList(name, user);
                if (ids == null) return null;
                result.put(user, ids);
            }
            return result;
        }

        public android.util.Pair<java.util.List<android.content.pm.VersionedPackage>, java.util.List<Boolean>> getPackagesUsingSharedLibrary(
                android.content.pm.SharedLibraryInfo library, long flags, int filterUid, int user) {
            var endpoint = endpoint();
            int uid = android.os.Binder.getCallingUid();
            int pid = android.os.Binder.getCallingPid();
            try {
                var users = endpoint.getSharedLibraryUsers(library.getName(), library.getLongVersion(), library.getType(), flags, filterUid, user, uid, pid);
                var optional = endpoint.getSharedLibraryUsersOptional(library.getName(), library.getLongVersion(), library.getType(), flags, filterUid, user, uid, pid);
                java.util.List<android.content.pm.VersionedPackage> packages = users == null ? null : new java.util.ArrayList<>(java.util.Arrays.asList(users));
                java.util.List<Boolean> options = null;
                if (optional != null) { options = new java.util.ArrayList<>(); for (boolean value : optional) options.add(value); }
                return new android.util.Pair<>(packages, options);
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public String getSetupWizardPackageName() {
            try { return endpoint().getSetupWizardPackageName(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public com.android.server.pm.pkg.AndroidPackage getPackage(int uid) {
            checkClosed();
            int user = android.os.UserHandle.getUserId(uid);
            boolean knownCompute = com.android.server.pm.NativeUserManagerBridge.isKnownIsolatedComputeApp(uid);
            int lookupUid;
            try { lookupUid = endpoint().getPackageLookupUid(uid, knownCompute,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            Object owner = uidOwner(android.os.UserHandle.getAppId(lookupUid));
            if (owner instanceof SharedUserApi group) {
                for (var state : group.getPackageStates()) {
                    if (!state.getUserStateOrDefault(user).isInstalled()
                            || shouldFilterApplication((PackageStateInternal) state, 1000, user, false)) continue;
                    var pkg = getRawPackage(state.getPackageName());
                    if (pkg != null) return pkg;
                }
            } else if (owner instanceof PackageStateInternal state
                    && state.getUserStateOrDefault(user).isInstalled()
                    && !shouldFilterApplication(state, 1000, user, false)) {
                return getRawPackage(state.getPackageName());
            }
            return null;
        }

        public android.util.ArrayMap<String, android.content.pm.ProcessInfo> getProcessesForUid(int uid) {
            android.content.pm.ProcessInfo[] values;
            try { values = endpoint().getProcessesForUid(uid, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            if (values == null) return null;
            var result = new android.util.ArrayMap<String, android.content.pm.ProcessInfo>();
            for (var value : values) {
                if (value == null) throw new IllegalStateException("process owner capture returned null entry");
                result.put(value.name, value);
            }
            return result;
        }

        public boolean shouldFilterApplication(PackageStateInternal state, int uid, int user, boolean uninstalled) {
            checkClosed();
            int kind = -1;
            String ownerName = null;
            String name = state == null ? null : state.getPackageName();
            int appId = state == null ? -1 : state.getAppId();
            if (state != null) {
                if (data.packages.get(name) == state) kind = 0;
                else if (data.disabled.get(name) == state) kind = 1;
                else if (data.uidOwners != null && data.uidOwners.get(appId) == state) kind = 2;
                else {
                    for (var group : data.sharedUsers.values()) for (var member : group.getPackageStates()) {
                        if (member == state) { kind = 3; ownerName = group.getName(); }
                    }
                }
                if (kind == -1) throw new IllegalArgumentException("package visibility candidate is outside retained capture");
            }
            try { return endpoint().shouldFilterApplication(kind, ownerName, name, appId, uid, user, uninstalled,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public com.android.server.utils.WatchedArrayMap<String, com.android.server.utils.WatchedLongSparseArray<android.content.pm.SharedLibraryInfo>> getSharedLibraries() {
            checkClosed();
            synchronized (data) { if (data.libraryRegistry != null) return data.libraryRegistry; }
            {
                android.content.pm.SharedLibraryInfo[] libraries;
                try { libraries = endpoint().getSharedLibraryRegistry(); }
                catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
                if (libraries == null) throw new IllegalStateException("shared library registry capture returned null");
                var result = new com.android.server.utils.WatchedArrayMap<String, com.android.server.utils.WatchedLongSparseArray<android.content.pm.SharedLibraryInfo>>();
                for (var library : libraries) {
                    if (library == null) throw new IllegalStateException("shared library registry entry is null");
                    var versions = result.get(library.getName());
                    if (versions == null) { versions = new com.android.server.utils.WatchedLongSparseArray<>(); result.put(library.getName(), versions); }
                    if (versions.get(library.getLongVersion()) != null) throw new IllegalStateException("duplicate shared library registry entry");
                    versions.put(library.getLongVersion(), library);
                }
                synchronized (data) {
                    if (data.libraryRegistry == null) data.libraryRegistry = result.snapshot();
                    return data.libraryRegistry;
                }
            }
        }

        public android.content.pm.SharedLibraryInfo getSharedLibraryInfo(String name, long version) {
            var versions = getSharedLibraries().get(name);
            return versions == null ? null : versions.get(version);
        }

        public boolean getBlockUninstall(int user, String name) {
            try { return endpoint().getBlockUninstall(user, name); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public com.android.server.pm.pkg.AndroidPackage getRawPackage(String name) {
            checkClosed();
            var state = data.packages.get(name);
            return state == null ? null : state.getAndroidPackage();
        }

        public boolean isApexPackage(String name) {
            checkClosed();
            var state = data.packages.get(name);
            return state != null && state.getAndroidPackage() != null && state.getAndroidPackage().isApex();
        }

        public java.util.List<PackageStateInternal> findSharedNonSystemLibraries(PackageStateInternal state) {
            checkClosed();
            var libraries = new java.util.ArrayList<android.content.pm.SharedLibraryInfo>();
            var names = new java.util.HashSet<String>();
            for (var library : state.getTransientState().getUsesLibraryInfos()) collectLibrary(library.getInfo(), libraries, names);
            var result = new java.util.ArrayList<PackageStateInternal>();
            for (var library : libraries) {
                var member = getPackageStateInternal(library.getPackageName());
                if (member != null && member.getPkg() != null) result.add(member);
            }
            return result;
        }

        // SharedLibraryUtils.findSharedLibraries, pinned AOSP (Apache 2.0), keeps dependency order and name deduplication.
        private static void collectLibrary(android.content.pm.SharedLibraryInfo library,
                java.util.List<android.content.pm.SharedLibraryInfo> result, java.util.Set<String> names) {
            if (!names.add(library.getName())) return;
            result.add(library);
            if (library.getDependencies() != null) for (var child : library.getDependencies()) collectLibrary(child, result, names);
        }

        public boolean isCallerInstallerOfRecord(com.android.server.pm.pkg.AndroidPackage pkg, int uid) {
            checkClosed();
            if (pkg == null) return false;
            var state = getPackageStateInternal(pkg.getPackageName());
            if (state == null) return false;
            var installer = getPackageStateInternal(com.android.server.pm.NativeComputer.installerPackageName(state));
            return installer != null && android.os.UserHandle.getAppId(installer.getAppId()) == android.os.UserHandle.getAppId(uid);
        }

        public int getPackageUidInternal(String name, long flags, int user, int filterUid) {
            try { return endpoint().getPackageUidWithCaller(name, flags, user, filterUid,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public String resolveInternalPackageName(String name, long version) {
            try { return endpoint().resolveInternalPackageName(name, version, android.os.Binder.getCallingUid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isCallerSameApp(String name, int uid, boolean isolated) {
            try { return endpoint().isCallerSameApp(name, uid, isolated,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public String getInstantAppPackageName(int uid) {
            try { return endpoint().getInstantAppPackageName(uid,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public int getComponentEnabledSetting(android.content.ComponentName component,
                int filterUid, int user, boolean internal) {
            try { return endpoint().getComponentEnabledSetting(component, filterUid, user, internal,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public java.util.List<android.content.pm.ApplicationInfo> getInstalledApplications(long flags,
                int user, int filterUid, boolean crossUser) {
            try {
                var result = endpoint().getInstalledApplications(flags, user, filterUid, crossUser,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
                if (result == null) throw new IllegalStateException("installed application capture returned null");
                return new java.util.ArrayList<android.content.pm.ApplicationInfo>(result.getList());
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isInstantAppInternal(String name, int user, int filterUid) {
            try { return endpoint().isInstantAppInternal(name, user, filterUid,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean canViewInstantApps(int filterUid, int user) {
            try { return endpoint().canViewInstantApps(filterUid, user,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public int checkUidSignaturesForAllUsers(int uid1, int uid2) {
            try { return endpoint().checkUidSignaturesForAllUsers(uid1, uid2,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public void enforceCrossUserPermission(int filterUid, int user, boolean full,
                boolean shell, String message) {
            try { endpoint().enforceCrossUserPermission(filterUid, user, full, shell, message,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public PackageStateInternal getPackageStateForInstalledAndFiltered(String name, int uid, int user) {
            var state = getPackageStateInternal(name);
            return state == null || shouldFilterApplication(state, uid, user, true) ? null : state;
        }

        private com.android.server.pm.pkg.PackageUserStateInternal userStateForUser(String name, int user)
                throws android.content.pm.PackageManager.NameNotFoundException {
            enforceCrossUserPermission(android.os.Binder.getCallingUid(), user, true, false,
                    "when asking about packages for user " + user);
            var state = (PackageStateInternal) data.packages.get(name);
            if (state == null || shouldFilterApplication(state, android.os.Binder.getCallingUid(), user, true))
                throw new android.content.pm.PackageManager.NameNotFoundException(name);
            return state.getUserStateOrDefault(user);
        }

        public boolean isPackageSuspendedForUser(String name, int user)
                throws android.content.pm.PackageManager.NameNotFoundException { return userStateForUser(name, user).isSuspended(); }
        public boolean isPackageStoppedForUser(String name, int user)
                throws android.content.pm.PackageManager.NameNotFoundException { return userStateForUser(name, user).isStopped(); }
        public boolean isPackageQuarantinedForUser(String name, int user)
                throws android.content.pm.PackageManager.NameNotFoundException { return userStateForUser(name, user).isQuarantined(); }

        public boolean isSuspendingAnyPackages(String name, int suspenderUser, int targetUser) {
            checkClosed();
            var suspender = android.content.pm.UserPackage.of(suspenderUser, name);
            for (var state : data.packages.values()) {
                var params = ((PackageStateInternal) state).getUserStateOrDefault(targetUser).getSuspendParams();
                if (params != null && params.containsKey(suspender)) return true;
            }
            return false;
        }

        public boolean isApplicationEffectivelyEnabled(String name, UserHandle user) {
            checkClosed();
            var state = data.packages.get(name);
            if (state == null) return false;
            int enabled = state.getUserStateOrDefault(user.getIdentifier()).getEnabledState();
            if (enabled == 0) { var pkg = getPackage(name); return pkg != null && pkg.isEnabled(); }
            return enabled == 1;
        }

        public boolean isComponentEffectivelyEnabled(android.content.pm.ComponentInfo component, UserHandle user) {
            checkClosed();
            var state = data.packages.get(component.packageName);
            if (state == null) return false;
            var userState = state.getUserStateOrDefault(user.getIdentifier());
            int enabled = userState.getEnabledState();
            if (enabled == 0 ? !component.applicationInfo.enabled : enabled != 1) return false;
            var className = component.getComponentName().getClassName();
            var enabledNames = userState.getEnabledComponents();
            if (enabledNames != null && enabledNames.contains(className)) return true;
            var disabledNames = userState.getDisabledComponents();
            if (disabledNames != null && disabledNames.contains(className)) return false;
            return component.isEnabled();
        }

        public android.util.ArraySet<String> getNotifyPackagesForReplacedReceived(String[] names) {
            var result = new android.util.ArraySet<String>();
            int uid = android.os.Binder.getCallingUid();
            int user = android.os.UserHandle.getUserId(uid);
            for (String name : names) {
                var state = getPackageStateInternal(name);
                if (!shouldFilterApplication(state, uid, user, false)) result.add(name);
            }
            return result;
        }

        public java.util.List<android.content.pm.ApplicationInfo> getPersistentApplications(boolean safeMode, int flags) {
            try {
                var result = endpoint().getPersistentApplications(safeMode, flags,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
                if (result == null) throw new IllegalStateException("persistent application capture returned null");
                return new java.util.ArrayList<>(java.util.Arrays.asList(result));
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public int getPackageStartability(boolean safeMode, String name, int filterCallingUid, int userId) {
            try { return endpoint().getPackageStartability(safeMode, name, filterCallingUid, userId,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ApplicationInfo getApplicationInfo(String name,
                long flags, int filterCallingUid, int userId) {
            try {
                return endpoint().getApplicationInfo(name, flags, userId, filterCallingUid,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.PackageInfo getPackageInfo(String name,
                long flags, int filterCallingUid, int userId) {
            try {
                return endpoint().getPackageInfo(name, flags, userId, filterCallingUid,
                        android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid());
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public android.content.pm.ApplicationInfo getApplicationInfoInternal(String name,
                long flags, int filterCallingUid, int userId) {
            return getApplicationInfo(name, flags, filterCallingUid, userId);
        }

        public android.content.pm.PackageInfo getPackageInfoInternal(String name,
                long versionCode, long flags, int filterCallingUid, int userId) {
            try { return endpoint().getPackageInfoInternal(name, versionCode, flags, userId,
                    filterCallingUid, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isSameApp(String name, int comparisonUid, int userId) {
            return isSameApp(name, 0, comparisonUid, userId);
        }

        public boolean isSameApp(String name, long flags, int comparisonUid, int userId) {
            try { return endpoint().isSameApp(name, flags, comparisonUid, userId,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean filterAppAccess(int uid, int callingUid) {
            try { return endpoint().filterUidAccess(uid, callingUid); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean canQueryPackage(int queryUid, String name) {
            try { return endpoint().canQueryPackage(queryUid, name,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean filterAppAccess(String name, int callingUid, int userId, boolean filterUninstalled) {
            return shouldFilterApplication(getPackageStateInternal(name), callingUid, userId, filterUninstalled);
        }

        public int getPackageUid(String name, long flags, int userId) {
            try { return endpoint().getPackageUid(name, flags, userId,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public String[] getPackagesForUid(int uid) {
            try { return endpoint().getPackagesForUid(uid,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public String getNameForUid(int uid) {
            try { return endpoint().getNameForUid(uid,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public boolean isInstantApp(String name, int userId) {
            try { return endpoint().isInstantApp(name, userId,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public int getTargetSdkVersion(String name) {
            try { return endpoint().getTargetSdkVersion(name,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        public String getInstallerPackageName(String name, int userId) {
            try { return endpoint().getInstallerPackageName(name, userId,
                    android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }

        @Override public void close() { super.close(); }
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
