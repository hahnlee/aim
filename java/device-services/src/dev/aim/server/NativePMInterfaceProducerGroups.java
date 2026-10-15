package dev.aim.server;

import android.content.Intent;
import android.os.Binder;
import com.android.server.pm.CrossProfileIntentFilter;
import com.android.server.pm.NativeComputer;
import com.android.server.pm.NativePreferredRecords;
import com.android.server.pm.PreferredIntentResolver;
import com.android.server.pm.snapshot.PackageDataSnapshot;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/** Producer adapters retain original object identities over one native capture. */
public final class NativePMInterfaceProducerGroups {
    private NativePMInterfaceProducerGroups() {}

    /** Native diagnostic protocol 1: title-state, text, then typed ProtoOutputStream operations. */
    public static NativeComputer computer(PackageSnapshots.ComputerSnapshot packages,
            NativeComputer.QueryOwner query, NativeComputer.ResolutionOwner resolution, PackagePolicyBridge policy) {
        return new NativeComputer(packages, query, resolution, new Diagnostics(packages), new Policy(policy));
    }

    public static NativeComputer computer(PackageSnapshots.ComputerSnapshot packages,
            long webPolicyVersion, Map<Integer, Boolean> capturedWebDisabled,
            NativeComputer.ResolutionOwner resolution, PackagePolicyBridge policy) {
        return new NativeComputer(packages,
                new NativePMPostResolutionFilter(packages, webPolicyVersion, capturedWebDisabled),
                resolution, new Diagnostics(packages), new Policy(policy));
    }

    public static NativeComputer computer(PackageSnapshots.ComputerSnapshot packages,long version,
            Map<Integer,Boolean> webDisabled,boolean safeMode,String platformPackage,android.content.Context context,
            com.android.server.pm.UserManagerInternal users,PackagePolicyBridge policy) {
        var resolution=new com.android.server.pm.NativeResolutionOwner(context,packages,version,safeMode,users,platformPackage);
        var result=computer(packages,version,webDisabled,resolution,policy);
        resolution.attach(result);return result;
    }

    /** Stable original type owners; mutable permission definitions and archiver are supplied by bootstrap. */
    public static final class Read implements NativePackageManagerInternal.ReadOwner {
        private final com.android.server.pm.permission.LegacyPermissionSettings permissions;
        private final NativePMLegacyRuntimePermissions runtime;
        private final com.android.server.pm.dex.DynamicCodeLogger dynamicCode;
        private final com.android.server.pm.PackageArchiver archiver;
        private final NativePMMutationProducer mutations;
        public Read(com.android.server.pm.permission.LegacyPermissionSettings permissions,
                NativePMLegacyRuntimePermissions runtime, com.android.server.pm.Installer installer,
                com.android.server.pm.PackageArchiver archiver, NativePMMutationProducer mutations) {
            this.permissions = Objects.requireNonNull(permissions);
            this.runtime = Objects.requireNonNull(runtime);
            this.dynamicCode = new com.android.server.pm.dex.DynamicCodeLogger(Objects.requireNonNull(installer));
            this.archiver = Objects.requireNonNull(archiver);
            this.mutations = Objects.requireNonNull(mutations);
        }
        public Read(PackageSnapshots.Store packages,NativePMLegacyRuntimePermissions runtime,
                com.android.server.pm.dex.DynamicCodeLogger dynamicCode,com.android.server.pm.PackageArchiver archiver,
                NativePMMutationProducer mutations) {
            this.permissions=Objects.requireNonNull(createLegacy(packages));this.runtime=Objects.requireNonNull(runtime);
            this.dynamicCode=Objects.requireNonNull(dynamicCode);this.archiver=Objects.requireNonNull(archiver);
            this.mutations=Objects.requireNonNull(mutations);
        }
        public Read(PackageSnapshots.Store packages, NativePMLegacyRuntimePermissions runtime,
                com.android.server.pm.Installer installer, com.android.server.pm.PackageArchiver archiver,
                NativePMMutationProducer mutations) {
            this(createLegacy(packages), runtime, installer, archiver, mutations);
        }
        public Read(android.content.Context context, PackageSnapshots.Store packages,
                NativePMLegacyRuntimePermissions runtime, com.android.server.pm.Installer installer,
                NativePMMutationProducer mutations, IPackageInternalHost host,
                java.util.function.Supplier<NativeComputer> computers, android.os.Handler handler) {
            this(packages, runtime, installer,
                    new com.android.server.pm.NativePackageArchiver(context, packages, host, computers, handler), mutations);
        }
        private static com.android.server.pm.permission.LegacyPermissionSettings createLegacy(PackageSnapshots.Store packages) {
            try (var scope = Objects.requireNonNull(packages).computer()) {
                return NativeLegacyPermissionSettings.create(scope);
            }
        }
        @Override public com.android.server.pm.permission.LegacyPermissionSettings getLegacyPermissions() { return permissions; }
        @Override public Object getLegacyPermissionsState(int user) { return runtime.state(user); }
        @Override public int getLegacyPermissionsVersion(int user) { return runtime.version(user); }
        @Override public com.android.server.pm.dex.DynamicCodeLogger getDynamicCodeLogger() { return dynamicCode; }
        @Override public android.content.pm.ParceledListSlice<android.content.pm.PackageInstaller.SessionInfo> getHistoricalSessions(int user) { return mutations.getHistoricalSessions(user); }
        @Override public com.android.server.pm.PackageArchiver getPackageArchiver() { return archiver; }
    }

    public static final class Diagnostics implements NativeComputer.DiagnosticsOwner {
        private final PackageSnapshots.ComputerSnapshot packages;
        public Diagnostics(PackageSnapshots.ComputerSnapshot packages) { this.packages = Objects.requireNonNull(packages); }
        private void emit(int kind, int type, String name, android.util.ArraySet<String> permissions,
                boolean checkIn, com.android.server.pm.DumpState state, java.io.FileDescriptor fd, java.io.PrintWriter text,
                android.util.proto.ProtoOutputStream proto) {
            byte[] options = state == null ? null : com.android.server.pm.NativeDumpStateRecord.capture(state);
            String[] names = permissions == null ? null : permissions.toArray(new String[0]);
            byte[] record = packages.getDiagnosticRecord(kind, type, name, names, checkIn, options);
            var parcel = android.os.Parcel.obtain();
            try {
                parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
                int version = parcel.readInt();
                if (version != 2) throw new IllegalStateException("diagnostic renderer record version differs");
                boolean title = parcel.readBoolean();
                String rendered = parcel.readString();
                if (text != null && rendered != null) text.print(rendered);
                byte[] fdPayload = parcel.createByteArray();
                if (fdPayload != null) {
                    if (text != null) text.flush();
                    try (var output = new java.io.BufferedOutputStream(new java.io.FileOutputStream(fd))) {
                        output.write(fdPayload); output.flush();
                    } catch (IllegalArgumentException | IllegalStateException | java.io.IOException failure) {
                        if (text == null) throw new IllegalStateException("diagnostic descriptor output failed", failure);
                        text.println("Failed writing: " + failure);
                    }
                }
                int count = parcel.readInt();
                if (count < 0) throw new IllegalStateException("diagnostic renderer operation count is negative");
                var tokens = new java.util.HashMap<Integer, Long>();
                for (int i = 0; i < count; i++) {
                    int op = parcel.readInt(); long field = parcel.readLong();
                    if (proto == null) throw new IllegalStateException("text diagnostic returned protobuf operation");
                    switch (op) {
                        case 0: { int id = parcel.readInt(); if (tokens.put(id, proto.start(field)) != null) throw new IllegalStateException("duplicate protobuf token"); break; }
                        case 1: { int id = parcel.readInt(); Long token = tokens.remove(id); if (token == null) throw new IllegalStateException("missing protobuf token"); proto.end(token); break; }
                        case 2: proto.write(field, parcel.readInt()); break;
                        case 3: proto.write(field, parcel.readLong()); break;
                        case 4: proto.write(field, parcel.readBoolean()); break;
                        case 5: proto.write(field, parcel.readString()); break;
                        case 6: proto.write(field, parcel.createByteArray()); break;
                        case 7: proto.write(field, parcel.readDouble()); break;
                        default: throw new IllegalStateException("unknown protobuf operation");
                    }
                }
                if (!tokens.isEmpty() || parcel.dataAvail() != 0) throw new IllegalStateException("diagnostic renderer record has unfinished data");
                if (state != null) state.setTitlePrinted(title);
            } finally { parcel.recycle(); }
        }
        @Override public void dump(int type, java.io.FileDescriptor fd, java.io.PrintWriter writer, com.android.server.pm.DumpState state) { emit(0,type,null,null,false,state,fd,writer,null); }
        @Override public void dumpPermissions(java.io.PrintWriter writer,String name,android.util.ArraySet<String> permissions,com.android.server.pm.DumpState state) { emit(1,0,name,permissions,false,state,null,writer,null); }
        @Override public void dumpPackages(java.io.PrintWriter writer,String name,android.util.ArraySet<String> permissions,com.android.server.pm.DumpState state,boolean checkIn) { emit(2,0,name,permissions,checkIn,state,null,writer,null); }
        @Override public void dumpKeySet(java.io.PrintWriter writer,String name,com.android.server.pm.DumpState state) { emit(3,0,name,null,false,state,null,writer,null); }
        @Override public void dumpSharedUsers(java.io.PrintWriter writer,String name,android.util.ArraySet<String> permissions,com.android.server.pm.DumpState state,boolean checkIn) { emit(4,0,name,permissions,checkIn,state,null,writer,null); }
        @Override public void dumpSharedUsersProto(android.util.proto.ProtoOutputStream proto) { emit(5,0,null,null,false,null,null,null,proto); }
        @Override public void dumpPackagesProto(android.util.proto.ProtoOutputStream proto) { emit(6,0,null,null,false,null,null,null,proto); }
        @Override public void dumpSharedLibrariesProto(android.util.proto.ProtoOutputStream proto) { emit(7,0,null,null,false,null,null,null,proto); }
    }

    public static final class Policy implements NativePackageManagerInternal.PolicyOwner,
            NativePackageManagerInternal.PolicyReadOwner {
        private final PackagePolicyBridge owner;
        public Policy(PackagePolicyBridge owner) { this.owner = Objects.requireNonNull(owner); }
        @Override public void setDeviceAndProfileOwnerPackages(int user, String deviceOwner, android.util.SparseArray<String> profiles) {
            owner.setDeviceAndProfileOwnerPackages(user, deviceOwner, profiles);
        }
        @Override public void setOwnerProtectedPackages(int user, List<String> names) { owner.setOwnerProtectedPackages(user, names); }
        @Override public void setExternalSourcesPolicy(android.content.pm.PackageManagerInternal.ExternalSourcesPolicy policy) {
            owner.setExternalSourcesPolicy(policy);
        }
        @Override public boolean isInstallDisabledForPackage(String name, int uid, int user) { return owner.isInstallDisabledForPackage(name, uid, user); }
        @Override public boolean isPackageDataProtected(int user, String name) { return owner.isPackageDataProtected(user, name); }
        @Override public boolean isPackageStateProtected(String name, int user) { return owner.isPackageStateProtected(name, user); }
    }

    public static final class Preferred implements AutoCloseable {
        private PackageSnapshots.ComputerSnapshot packages;
        private PackageDataSnapshot view;
        private final Map<Integer, NativePreferredRecords.Scope> users = new HashMap<>();

        public Preferred(PackageSnapshots.ComputerSnapshot packages, PackageDataSnapshot view) {
            this.packages = Objects.requireNonNull(packages);
            this.view = Objects.requireNonNull(view);
        }
        private NativePreferredRecords.Scope scope(int user) {
            if (packages == null) throw new IllegalStateException("preferred producer scope closed");
            packages.getVersion();
            NativePreferredRecords.Scope current = users.get(user);
            if (current != null) return current;
            var preferred = packages.getPreferredRecordTokens(user, NativePreferredRecords.PREFERRED);
            var persistent = packages.getPreferredRecordTokens(user, NativePreferredRecords.PERSISTENT);
            var cross = packages.getPreferredRecordTokens(user, NativePreferredRecords.CROSS_PROFILE);
            long identity = Binder.clearCallingIdentity();
            try {
                current = NativePreferredRecords.capture(view, preferred, persistent, cross);
                users.put(user, current);
                return current;
            } finally { Binder.restoreCallingIdentity(identity); }
        }
        public synchronized android.content.pm.ResolveInfo findPersistent(Intent intent, String type,
                long flags, java.util.List<android.content.pm.ResolveInfo> query, int user) {
            return scope(user).findPersistent(intent, type, flags, query, (com.android.server.pm.Computer) view, user);
        }
        public synchronized boolean isDpmPreferred(Intent intent, String type, long flags, int user) {
            return scope(user).isDpmPreferred(intent, type, flags, user);
        }
        public synchronized PreferredIntentResolver preferred(int user) { return scope(user).preferred(); }
        public synchronized List<CrossProfileIntentFilter> crossProfile(Intent intent, String type, int user) {
            return scope(user).matchingCrossProfile(intent, type, user);
        }
        @Override public synchronized void close() {
            for (var scope : users.values()) scope.close();
            users.clear(); packages = null; view = null;
        }
    }
}
