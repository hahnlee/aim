package com.android.server.pm;

import android.content.ComponentName;
import android.content.IntentFilter;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Process;
import dev.aim.server.IPackageResolverIdentity;
import com.android.server.pm.snapshot.PackageDataSnapshot;
import java.util.ArrayList;
import java.util.IdentityHashMap;
import java.util.List;

/** Original typed records, retained by their native registry's Binder leases. */
public final class NativePreferredRecords {
    public static final int FORMAT = 1;
    public static final int PREFERRED = 0;
    public static final int PERSISTENT = 1;
    public static final int CROSS_PROFILE = 2;
    private NativePreferredRecords() {}

    private static final class PreferredRecord extends PreferredActivity {
        PreferredRecord(IntentFilter filter, int match, ComponentName[] set, ComponentName activity,
                boolean always) { super(filter, match, set, activity, always); seal(); }
        @Override public PreferredActivity snapshot() { return this; }
    }
    private static final class PersistentRecord extends PersistentPreferredActivity {
        PersistentRecord(IntentFilter filter, ComponentName activity, boolean byDpm) {
            super(filter, activity, byDpm); seal();
        }
        @Override public PersistentPreferredActivity snapshot() { return this; }
    }
    private static final class CrossRecord extends CrossProfileIntentFilter {
        CrossRecord(IntentFilter filter, String owner, int target, int flags, int access) {
            super(filter, owner, target, flags, access); seal();
        }
        @Override public CrossProfileIntentFilter snapshot() { return this; }
    }

    private static final class Lease extends IPackageResolverIdentity.Stub {
        final int kind;
        final WatchedIntentFilter record;
        Lease(int kind, WatchedIntentFilter record) { this.kind = kind; this.record = record; }
        @Override public int getIdentityHash() {
            enforceSystem();
            return System.identityHashCode(record);
        }
    }
    private static void enforceSystem() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("native preferred record identities require system UID");
        }
    }

    /** Payload is an original Parcel, never XML, so constructor semantics and
     * PersistableBundle/PatternMatcher data survive without reparsing. */
    public static IPackageResolverIdentity allocate(byte[] bytes) {
        enforceSystem();
        if (bytes == null) throw new NullPointerException("preferred record payload");
        Parcel parcel = Parcel.obtain();
        try {
            parcel.unmarshall(bytes, 0, bytes.length);
            parcel.setDataPosition(0);
            if (parcel.readInt() != FORMAT) throw new IllegalArgumentException("preferred record version");
            int kind = parcel.readInt();
            IntentFilter filter = parcel.readTypedObject(IntentFilter.CREATOR);
            if (filter == null) throw new NullPointerException("preferred record filter");
            WatchedIntentFilter record;
            switch (kind) {
                case PREFERRED: {
                    int match = parcel.readInt();
                    ComponentName[] set = parcel.createTypedArray(ComponentName.CREATOR);
                    ComponentName activity = parcel.readTypedObject(ComponentName.CREATOR);
                    boolean always = parcel.readBoolean();
                    record = new PreferredRecord(filter, match, set, activity, always);
                    break;
                }
                case PERSISTENT: {
                    ComponentName activity = parcel.readTypedObject(ComponentName.CREATOR);
                    boolean byDpm = parcel.readBoolean();
                    record = new PersistentRecord(filter, activity, byDpm);
                    break;
                }
                case CROSS_PROFILE: {
                    String owner = parcel.readString();
                    int target = parcel.readInt(), flags = parcel.readInt(), access = parcel.readInt();
                    record = new CrossRecord(filter, owner, target, flags, access);
                    break;
                }
                default: throw new IllegalArgumentException("preferred record kind " + kind);
            }
            parcel.enforceNoDataAvail();
            return new Lease(kind, record);
        } finally { parcel.recycle(); }
    }

    private static Lease retained(IBinder token, int kind) {
        if (token == null) throw new NullPointerException("preferred record lease");
        var identity = IPackageResolverIdentity.Stub.asInterface(token);
        // The lease returns to the original process that allocated the record.
        // A foreign/proxy identity cannot supply an original local record.
        if (!(identity instanceof Lease)) throw new IllegalArgumentException("foreign preferred record lease");
        Lease lease = (Lease) identity;
        if (lease.kind != kind) throw new IllegalArgumentException("preferred record lease kind");
        return lease;
    }

    /** One captured user's typed resolvers. Repeated getters return the same
     * resolver objects, and every entry reuses the registry's allocated record.
     * Original snapshot() calls also retain the same sealed record identities. */
    public static final class Scope implements AutoCloseable {
        private List<Lease> leases;
        private PackageDataSnapshot snapshot;
        private PreferredIntentResolver preferred;
        private PersistentPreferredIntentResolver persistent;
        private CrossProfileIntentResolver cross;
        private Scope(PackageDataSnapshot snapshot, IBinder[] preferredTokens,
                IBinder[] persistentTokens, IBinder[] crossTokens) {
            this.snapshot = snapshot;
            leases = new ArrayList<>();
            IdentityHashMap<Lease, Boolean> seen = new IdentityHashMap<>();
            if (preferredTokens != null) {
                PreferredIntentResolver resolver = new PreferredIntentResolver();
                for (IBinder token : preferredTokens) {
                    Lease lease = retained(token, PREFERRED);
                    if (seen.put(lease, Boolean.TRUE) != null) throw new IllegalArgumentException("duplicate preferred record lease");
                    leases.add(lease); resolver.addFilter(snapshot, (PreferredActivity) lease.record);
                }
                preferred = resolver.snapshot();
            }
            if (persistentTokens != null) {
                PersistentPreferredIntentResolver resolver = new PersistentPreferredIntentResolver();
                for (IBinder token : persistentTokens) {
                    Lease lease = retained(token, PERSISTENT);
                    if (seen.put(lease, Boolean.TRUE) != null) throw new IllegalArgumentException("duplicate persistent record lease");
                    leases.add(lease); resolver.addFilter(snapshot, (PersistentPreferredActivity) lease.record);
                }
                persistent = resolver.snapshot();
            }
            if (crossTokens != null) {
                CrossProfileIntentResolver resolver = new CrossProfileIntentResolver();
                for (IBinder token : crossTokens) {
                    Lease lease = retained(token, CROSS_PROFILE);
                    if (seen.put(lease, Boolean.TRUE) != null) throw new IllegalArgumentException("duplicate cross-profile record lease");
                    leases.add(lease); resolver.addFilter(snapshot, (CrossProfileIntentFilter) lease.record);
                }
                cross = resolver.snapshot();
            }
        }
        private void open() { if (leases == null) throw new IllegalStateException("preferred record scope closed"); }
        public synchronized PreferredIntentResolver preferred() { open(); return preferred; }
        public synchronized PersistentPreferredIntentResolver persistent() { open(); return persistent; }
        public synchronized android.content.pm.ResolveInfo findPersistent(android.content.Intent intent,
                String type, long flags, java.util.List<android.content.pm.ResolveInfo> query,
                Computer computer, int user) {
            open();
            if (persistent == null) return null;
            var matches = ((com.android.server.IntentResolver<PersistentPreferredActivity,PersistentPreferredActivity>) persistent).queryIntent(snapshot, intent, type, (flags & 0x10000L) != 0, user);
            for (var record : matches) {
                var activity = computer.getActivityInfo(record.mComponent, flags | 0x200L, user);
                if (activity == null) continue;
                for (var candidate : query) {
                    if (candidate.activityInfo.applicationInfo.packageName.equals(activity.applicationInfo.packageName)
                            && candidate.activityInfo.name.equals(activity.name)) return candidate;
                }
            }
            return null;
        }
        public synchronized boolean isDpmPreferred(android.content.Intent intent, String type,
                long flags, int user) {
            open();
            if (persistent == null) return false;
            var matches = ((com.android.server.IntentResolver<PersistentPreferredActivity,PersistentPreferredActivity>) persistent).queryIntent(snapshot, intent, type, (flags & 0x10000L) != 0, user);
            for (var record : matches) if (record.mIsSetByDpm) return true;
            return false;
        }
        synchronized CrossProfileIntentResolver crossProfile() { open(); return cross; }
        public synchronized List<CrossProfileIntentFilter> matchingCrossProfile(android.content.Intent intent,
                String resolvedType, int user) {
            open();
            return cross == null ? null : ((com.android.server.IntentResolver<CrossProfileIntentFilter,CrossProfileIntentFilter>) cross).queryIntent(snapshot, intent, resolvedType, false, user);
        }
        @Override public synchronized void close() {
            leases = null; snapshot = null; preferred = null; persistent = null; cross = null;
        }
    }
    public static Scope capture(PackageDataSnapshot snapshot, IBinder[] preferred,
            IBinder[] persistent, IBinder[] cross) {
        return new Scope(snapshot, preferred, persistent, cross);
    }
}
