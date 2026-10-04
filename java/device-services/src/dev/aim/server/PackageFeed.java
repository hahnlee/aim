package dev.aim.server;

import android.app.AppGlobals;
import android.app.role.RoleManager;
import android.content.ComponentName;
import android.content.Context;
import android.content.UriRelativeFilter;
import android.content.UriRelativeFilterGroup;
import android.content.pm.ApplicationInfo;
import android.content.pm.IPackageManager;
import android.content.pm.InstallSourceInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.PermissionGroupInfo;
import android.content.pm.PermissionInfo;
import android.content.pm.ProviderInfo;
import android.content.pm.Signature;
import android.content.pm.SigningDetails;
import android.content.pm.SigningInfo;
import android.content.pm.overlay.OverlayPaths;
import android.content.res.Resources;
import android.database.ContentObserver;
import android.content.pm.verify.domain.DomainVerificationInfo;
import android.content.pm.verify.domain.DomainVerificationManager;
import android.content.pm.verify.domain.DomainVerificationUserState;
import android.os.Bundle;
import android.os.Environment;
import android.os.FileObserver;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IRemoteCallback;
import android.os.Parcel;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.permission.PermissionManager;
import android.provider.DeviceConfig;
import android.provider.Settings;
import android.util.Slog;
import android.util.SparseArray;

import com.android.internal.compat.IPlatformCompat;
import com.android.internal.pm.parsing.pkg.ParsedPackage;
import com.android.internal.pm.pkg.component.ParsedPermission;
import com.android.internal.pm.pkg.component.ParsedProvider;
import com.android.server.LocalManagerRegistry;
import com.android.server.pm.PackageManagerLocal;
import com.android.server.pm.parsing.PackageCacher;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.ArchiveState;
import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.PackageUserState;
import com.android.server.pm.pkg.SharedUserApi;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.PublicKey;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;

/**
 * The feed of the original PackageManager's state to the native
 * PackageManager's model in the service host (docs/m4-packagemanager.md,
 * slice A): at attach a snapshot through PackageManagerLocal and the
 * system APIs, then, on each package monitor callback and each sync the
 * host asks for when the package_info_cache nonce moved, the records that
 * changed. Every batch ends with the digest of a fresh snapshot, which the
 * host checks its records against. It runs on a thread of its own and
 * never under a lock of PackageManager's.
 *
 * The records are Parcels in the layout feed.rs reads; a parsed package
 * is PackageImpl as the parser cache keeps it (PackageCacher), sent again
 * when its content hash changes, including mutations of the same object.
 */
final class PackageFeed extends IPackageFeed.Stub {
    private static final String TAG = "AimPackageFeed";

    /** The records' kinds, as feed.rs numbers them. */
    private static final int PACKAGE = 0;
    private static final int DISABLED_SYSTEM_PACKAGE = 1;
    private static final int PARSED = 2;
    private static final int DISABLED_SYSTEM_PARSED = 3;
    private static final int SHARED_USER = 4;
    private static final int USER = 5;
    private static final int SYSTEM = 6;
    private static final int RUNTIME = 7;
    private static final int DISABLED_SYSTEM_RUNTIME = 8;
    private static final int SHARED_PROCESSES = 9;
    private static final int USER_SCOPE = 10;
    private static final int DISABLED_SYSTEM_USER_SCOPE = 11;
    private static final int SCAN_USERS = 12;
    /** The largest chunk of a record in one transaction. */
    private static final int CHUNK = 128 * 1024;
    /** UserHandle.USER_ALL. */
    private static final int USER_ALL = -1;
    /** PackageManager.FILTER_APPLICATION_QUERY. */
    private static final long FILTER_APPLICATION_QUERY = 135549675L;
    /** AppsFilterImpl.FeatureConfigImpl's DeviceConfig flag. */
    private static final String FILTERING_ENABLED = "package_query_filtering_enabled";
    /** VirtualDeviceManager.PERSISTENT_DEVICE_ID_DEFAULT. */
    private static final String DEVICE_DEFAULT = "default:0";

    private final Context mContext;
    private final Object mLock = new Object();
    private Handler mHandler;
    /** Kept so that it keeps watching. */
    private FileObserver mCompatOverrides;
    private boolean mScheduled;
    private boolean mReset;
    /** The latest sync's token. */
    private long mToken;

    // The feed thread's state.
    private IPackageFeedHost mHost;
    /** What the host holds: each record's SHA-256, by kind and key. */
    private final TreeMap<Key, byte[]> mSent = new TreeMap<>();
    /**
     * Syncable providers' declared authorities, by the AndroidPackage they
     * were read for: this batch's and the previous one's.
     */
    private IdentityHashMap<AndroidPackage, Map<String, String>> mDeclared =
            new IdentityHashMap<>();
    private IdentityHashMap<AndroidPackage, Map<String, String>> mDeclaredBefore;
    /** This batch's installed permission definitions, by owner package. */
    private Map<String, TreeSet<String>> mInstalledPermissions;

    PackageFeed(Context context) {
        mContext = context;
    }

    /** IBridge.getPackageFeed: feeds `host` from now on, starting with a snapshot. */
    IPackageFeed attach(IPackageFeedHost host) {
        synchronized (mLock) {
            if (mHandler == null) {
                HandlerThread thread = new HandlerThread(TAG);
                thread.start();
                mHandler = thread.getThreadHandler();
                registerPackageMonitor();
                registerDeviceProvisioned();
                registerRoleHolders();
                registerQueryFiltering();
            }
            mHandler.post(() -> mHost = host);
        }
        sync(true, 0);
        return this;
    }

    @Override
    public void sync(boolean reset, long token) {
        synchronized (mLock) {
            mReset |= reset;
            mToken = Math.max(mToken, token);
            if (mHandler == null || mScheduled) {
                return;
            }
            mScheduled = true;
            mHandler.post(this::batch);
        }
    }

    /** IPackageManager.registerPackageMonitorCallback: a delta per package broadcast. */
    private void registerPackageMonitor() {
        try {
            AppGlobals.getPackageManager().registerPackageMonitorCallback(
                    new IRemoteCallback.Stub() {
                        @Override
                        public void sendResult(Bundle data) {
                            sync(false, 0);
                        }
                    }, USER_ALL);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
    }

    /**
     * A batch when the device becomes provisioned: preferred activity
     * resolution reads Settings.Global.DEVICE_PROVISIONED.
     */
    private void registerDeviceProvisioned() {
        mContext.getContentResolver().registerContentObserver(
                Settings.Global.getUriFor(Settings.Global.DEVICE_PROVISIONED), false,
                new ContentObserver(mHandler) {
                    @Override
                    public void onChange(boolean selfChange) {
                        sync(false, 0);
                    }
                });
    }

    /** A batch when a role's holders change: resolution reads the default browser. */
    private void registerRoleHolders() {
        mContext.getSystemService(RoleManager.class).addOnRoleHoldersChangedListenerAsUser(
                r -> mHandler.post(r), (role, user) -> sync(false, 0), UserHandle.ALL);
    }

    /**
     * A batch when package query filtering may have changed: its
     * DeviceConfig flag, or platform compat's overrides, which it saves
     * to compat_framework_overrides.xml on each change and no API reports.
     */
    private void registerQueryFiltering() {
        DeviceConfig.addOnPropertiesChangedListener(
                DeviceConfig.NAMESPACE_PACKAGE_MANAGER_SERVICE, r -> mHandler.post(r),
                properties -> sync(false, 0));
        mCompatOverrides = new FileObserver(
                new File(Environment.getDataMiscDirectory(), "appcompat"),
                FileObserver.CLOSE_WRITE | FileObserver.MOVED_TO) {
            @Override
            public void onEvent(int event, String path) {
                sync(false, 0);
            }
        };
        mCompatOverrides.startWatching();
    }

    /** One batch: the records that differ from what the host holds. */
    private void batch() {
        boolean reset;
        long token;
        synchronized (mLock) {
            mScheduled = false;
            reset = mReset;
            mReset = false;
            token = mToken;
        }
        IPackageFeedHost host = mHost;
        if (host == null) {
            return;
        }
        try {
            send(host, reset, token);
        } catch (RemoteException e) {
            Slog.w(TAG, "the package feed's host died", e);
            mHost = null;
            mSent.clear();
        } catch (RuntimeException e) {
            // What the host holds is unknown now: the next batch sends all.
            Slog.e(TAG, "a package feed batch failed", e);
            synchronized (mLock) {
                mReset = true;
            }
        }
    }

    private void send(IPackageFeedHost host, boolean reset, long token) throws RemoteException {
        if (reset) {
            mSent.clear();
        }
        TreeMap<Key, byte[]> records = new TreeMap<>();
        TreeSet<Integer> users = new TreeSet<>();
        mDeclaredBefore = mDeclared;
        mDeclared = new IdentityHashMap<>();
        PackageManagerLocal local = LocalManagerRegistry.getManager(PackageManagerLocal.class);
        try (PackageManagerLocal.UnfilteredSnapshot snapshot = local.withUnfilteredSnapshot()) {
            mInstalledPermissions = installedPermissions(snapshot.getPackageStates().values());
            for (PackageState state : snapshot.getPackageStates().values()) {
                add(records, users, PACKAGE, PARSED, state);
                records.put(new Key(USER_SCOPE, state.getPackageName()), PackageUserScopeFeed.capture(state, null, false));
            }
            for (PackageState state : snapshot.getDisabledSystemPackageStates().values()) {
                add(records, users, DISABLED_SYSTEM_PACKAGE, DISABLED_SYSTEM_PARSED,
                        state);
                records.put(new Key(DISABLED_SYSTEM_USER_SCOPE, state.getPackageName()), PackageUserScopeFeed.capture(state, snapshot.getPackageStates().get(state.getPackageName()), true));
            }
            for (SharedUserApi user : snapshot.getSharedUsers().values()) {
                records.put(new Key(SHARED_USER, user.getName()), sharedUser(user));
                records.put(new Key(SHARED_PROCESSES, user.getName()), com.android.server.pm.SharedProcessFeed.capture(user));
            }
        }
        for (int user : users) {
            records.put(new Key(USER, Integer.toString(user)), user(user));
        }
        records.put(new Key(SYSTEM, ""), system());
        records.put(new Key(SCAN_USERS, ""), PackageScanUsers.capture());
        host.begin(reset);
        for (Map.Entry<Key, byte[]> e : records.entrySet()) {
            byte[] hash = sha256(e.getValue());
            if (!Arrays.equals(hash, mSent.get(e.getKey()))) {
                put(host, e.getKey(), e.getValue());
                mSent.put(e.getKey(), hash);
            }
        }
        for (Key key : new ArrayList<>(mSent.keySet())) {
            if (!records.containsKey(key)) {
                host.remove(key.kind, key.name);
                mSent.remove(key);
            }
        }
        host.end(digest(), token);
    }

    private void add(Map<Key, byte[]> records, Set<Integer> users,
            int kind, int parsedKind, PackageState state) {
        String name = state.getPackageName();
        records.put(new Key(kind, name), packageState(state, kind == PACKAGE));
        records.put(new Key(kind == PACKAGE ? RUNTIME : DISABLED_SYSTEM_RUNTIME, name),
                PackageRuntimeFeed.capture(state));
        AndroidPackage pkg = state.getAndroidPackage();
        if (pkg instanceof ParsedPackage parsed) {
            // PackageImpl fields can change while the original retains its identity.
            records.put(new Key(parsedKind, name), PackageCacher.toCacheEntryStatic(parsed));
        }
        SparseArray<? extends PackageUserState> states = state.getUserStates();
        for (int i = 0; i < states.size(); i++) {
            users.add(states.keyAt(i));
        }
    }

    private static void put(IPackageFeedHost host, Key key, byte[] record) throws RemoteException {
        int at = 0;
        do {
            int end = Math.min(record.length, at + CHUNK);
            host.put(key.kind, key.name, record.length,
                    at == 0 && end == record.length ? record : Arrays.copyOfRange(record, at, end));
            at = end;
        } while (at < record.length);
    }

    /**
     * The SHA-256 of the records the host holds: per record in key order
     * its kind, its key's length and UTF-8 bytes, and its own SHA-256.
     */
    private byte[] digest() {
        MessageDigest digest = sha256();
        for (Map.Entry<Key, byte[]> e : mSent.entrySet()) {
            byte[] name = e.getKey().name.getBytes(StandardCharsets.UTF_8);
            digest.update(int32(e.getKey().kind));
            digest.update(int32(name.length));
            digest.update(name);
            digest.update(e.getValue());
        }
        return digest.digest();
    }

    private static byte[] int32(int v) {
        return new byte[] {(byte) (v >> 24), (byte) (v >> 16), (byte) (v >> 8), (byte) v};
    }

    private static MessageDigest sha256() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    private static byte[] sha256(byte[] bytes) {
        return sha256().digest(bytes);
    }

    // The records. Each is a Parcel: strings as writeString, lists as
    // their size (-1 for null) and elements, booleans in flag words.

    private interface Writer {
        void write(Parcel p);
    }

    private static byte[] record(Writer writer) {
        Parcel p = Parcel.obtain();
        try {
            writer.write(p);
            return p.marshall();
        } finally {
            p.recycle();
        }
    }

    /**
     * A package state's record; `installed`: the package the name stands
     * for now, not the system package an update replaced, whose install
     * source no API gives (#714).
     */
    private byte[] packageState(PackageState s, boolean installed) {
        return record(p -> {
            p.writeString(s.getPackageName());
            p.writeInt(s.getAppId());
            p.writeInt(s.getSharedUserAppId());
            File path = s.getPath();
            p.writeString(path == null ? null : path.getPath());
            p.writeString(s.getVolumeUuid());
            p.writeString(s.getPrimaryCpuAbi());
            p.writeString(s.getSecondaryCpuAbi());
            p.writeString(s.getCpuAbiOverride());
            p.writeString(s.getSeInfo());
            p.writeString(s.getApexModuleName());
            p.writeLong(s.getVersionCode());
            p.writeInt(s.getTargetSdkVersion());
            p.writeInt(s.getCategoryOverride());
            p.writeInt(s.getHiddenApiEnforcementPolicy());
            p.writeLong(s.getLastModifiedTime());
            p.writeLong(s.getLastUpdateTime());
            p.writeByteArray(s.getRestrictUpdateHash());
            p.writeInt(flags(s.hasSharedUser(), s.isApex(), s.isApkInUpdatedApex(),
                    s.isDebuggable(), s.isDefaultToDeviceProtectedStorage(),
                    s.isExternalStorage(), s.isForceQueryableOverride(),
                    s.isHiddenUntilInstalled(), s.isInstallPermissionsFixed(),
                    s.isLeavingSharedUser(), s.isOdm(), s.isOem(),
                    s.isPageSizeAppCompatEnabled(), s.isPendingRestore(), s.isPersistent(),
                    s.isPrivileged(), s.isProduct(), s.isRequiredForSystemUser(),
                    s.isScannedAsStoppedSystemApp(), s.isSystem(), s.isSystemExt(),
                    s.isUpdateAvailable(), s.isUpdatedSystemApp(), s.isVendor()));
            PackageMimeGroups.write(p, s.getMimeGroups());
            String[] staticLibraries = s.getUsesStaticLibraries();
            long[] staticVersions = s.getUsesStaticLibrariesVersions();
            p.writeInt(staticLibraries.length);
            for (int i = 0; i < staticLibraries.length; i++) {
                p.writeString(staticLibraries[i]);
                p.writeLong(staticVersions[i]);
            }
            String[] sdkLibraries = s.getUsesSdkLibraries();
            long[] sdkVersions = s.getUsesSdkLibrariesVersionsMajor();
            boolean[] sdkOptional = s.getUsesSdkLibrariesOptional();
            p.writeInt(sdkLibraries.length);
            for (int i = 0; i < sdkLibraries.length; i++) {
                p.writeString(sdkLibraries[i]);
                p.writeLong(sdkVersions[i]);
                p.writeBoolean(sdkOptional[i]);
            }
            strings(p, s.getUsesLibraryFiles());
            PackageLibraryFeed.write(p, s.getSharedLibraryDependencies());
            strings(p, mInstalledPermissions.get(s.getPackageName()));
            signing(p, s.getSigningInfo());
            installSource(p, installed ? s : null);
            DomainVerificationInfo domains = domainVerification(s.getPackageName());
            p.writeBoolean(domains != null);
            if (domains != null) {
                p.writeString(domains.getIdentifier().toString());
                hostStates(p, domains.getHostToStateMap());
            }
            SparseArray<? extends PackageUserState> users = s.getUserStates();
            p.writeInt(users.size());
            for (int i = 0; i < users.size(); i++) {
                p.writeInt(users.keyAt(i));
                userState(p, users.valueAt(i), s.getPackageName(), users.keyAt(i));
            }
            uriRelativeFilterGroups(p, s.getPackageName(), users);
            p.writeBoolean(filterApplicationQuery(s));
            syncableAuthorities(p, s.getAndroidPackage());
        });
    }

    /**
     * The URI relative filter groups domain verification keeps for the
     * package's web domains (the hosts of its users' selections), by
     * domain.
     */
    private void uriRelativeFilterGroups(Parcel p, String packageName,
            SparseArray<? extends PackageUserState> users) {
        TreeSet<String> hosts = new TreeSet<>();
        for (int i = 0; i < users.size(); i++) {
            DomainVerificationUserState selection = domainSelection(packageName, users.keyAt(i));
            if (selection != null) {
                hosts.addAll(selection.getHostToStateMap().keySet());
            }
        }
        Map<String, List<UriRelativeFilterGroup>> groups = hosts.isEmpty() ? Map.of()
                : mContext.getSystemService(DomainVerificationManager.class)
                        .getUriRelativeFilterGroups(packageName, new ArrayList<>(hosts));
        TreeMap<String, List<UriRelativeFilterGroup>> sorted = new TreeMap<>(groups);
        p.writeInt(sorted.size());
        for (Map.Entry<String, List<UriRelativeFilterGroup>> e : sorted.entrySet()) {
            p.writeString(e.getKey());
            p.writeInt(e.getValue().size());
            for (UriRelativeFilterGroup group : e.getValue()) {
                p.writeInt(group.getAction());
                p.writeInt(group.getUriRelativeFilters().size());
                for (UriRelativeFilter f : group.getUriRelativeFilters()) {
                    p.writeInt(f.getUriPart());
                    p.writeInt(f.getPatternType());
                    p.writeString(f.getFilter());
                }
            }
        }
    }

    /**
     * The authorities the manifest declares for each syncable provider
     * with several, by provider: registration (ComponentResolver's
     * addProvidersLocked) leaves the first in the package's provider and
     * gives the rest to a copy that is not in the package. Read from the
     * base APK once per AndroidPackage.
     */
    private void syncableAuthorities(Parcel p, AndroidPackage pkg) {
        Map<String, String> declared = Map.of();
        if (pkg != null && hasSyncableProvider(pkg)) {
            declared = mDeclaredBefore.get(pkg);
            if (declared == null) {
                declared = new TreeMap<>();
                PackageInfo info = mContext.getPackageManager().getPackageArchiveInfo(
                        pkg.getBaseApkPath(), PackageManager.GET_PROVIDERS
                                | PackageManager.MATCH_DISABLED_COMPONENTS
                                | PackageManager.MATCH_DIRECT_BOOT_AWARE
                                | PackageManager.MATCH_DIRECT_BOOT_UNAWARE);
                if (info == null) {
                    Slog.w(TAG, "cannot read the providers of " + pkg.getBaseApkPath());
                } else if (info.providers != null) {
                    for (ProviderInfo provider : info.providers) {
                        if (provider.isSyncable && provider.authority != null
                                && provider.authority.indexOf(';') >= 0) {
                            declared.put(provider.name, provider.authority);
                        }
                    }
                }
            }
            mDeclared.put(pkg, declared);
        }
        p.writeInt(declared.size());
        for (Map.Entry<String, String> e : declared.entrySet()) {
            p.writeString(e.getKey());
            p.writeString(e.getValue());
        }
    }

    private static boolean hasSyncableProvider(AndroidPackage pkg) {
        for (ParsedProvider provider : pkg.getProviders()) {
            if (provider.isSyncable()) {
                return true;
            }
        }
        return false;
    }

    /**
     * Whether FILTER_APPLICATION_QUERY is on for the package, as
     * AppsFilterImpl's FeatureConfig asks platform compat (with the
     * package's name and target SDK, all the change reads).
     */
    private static boolean filterApplicationQuery(PackageState s) {
        ApplicationInfo info = new ApplicationInfo();
        info.packageName = s.getPackageName();
        info.targetSdkVersion = s.getTargetSdkVersion();
        try {
            return IPlatformCompat.Stub.asInterface(
                    ServiceManager.getService(Context.PLATFORM_COMPAT_SERVICE))
                    .getAppConfig(info).isChangeEnabled(FILTER_APPLICATION_QUERY);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
    }

    /** The installed permission definitions, by the package that defines each. */
    private Map<String, TreeSet<String>> installedPermissions(Collection<PackageState> packages) {
        PackageManager pm = mContext.getPackageManager();
        List<String> groups = new ArrayList<>();
        groups.add(null);
        for (PermissionGroupInfo group : pm.getAllPermissionGroups(0)) {
            groups.add(group.name);
        }
        HashMap<String, TreeSet<String>> installed = new HashMap<>();
        for (String group : groups) {
            List<PermissionInfo> permissions;
            try {
                permissions = pm.queryPermissionsByGroup(group, 0);
            } catch (PackageManager.NameNotFoundException e) {
                continue;
            }
            for (PermissionInfo permission : permissions) {
                installed.computeIfAbsent(permission.packageName, k -> new TreeSet<>())
                        .add(permission.name);
            }
        }
        // Group queries can omit installed definitions.
        for (PackageState state : packages) {
            AndroidPackage pkg = state.getAndroidPackage();
            if (pkg == null) {
                continue;
            }
            for (ParsedPermission permission : pkg.getPermissions()) {
                String name = permission.getName();
                TreeSet<String> names = installed.get(state.getPackageName());
                if (names != null && names.contains(name)) {
                    continue;
                }
                try {
                    PermissionInfo info = pm.getPermissionInfo(name, 0);
                    if (state.getPackageName().equals(info.packageName)) {
                        installed.computeIfAbsent(info.packageName, k -> new TreeSet<>()).add(name);
                    }
                } catch (PackageManager.NameNotFoundException e) {
                    // A declared permission need not be the installed definition.
                }
            }
        }
        return installed;
    }

    private static int flags(boolean... bits) {
        int flags = 0;
        for (int i = 0; i < bits.length; i++) {
            if (bits[i]) {
                flags |= 1 << i;
            }
        }
        return flags;
    }

    private static void strings(Parcel p, Collection<String> strings) {
        p.writeInt(strings == null ? -1 : strings.size());
        if (strings != null) {
            for (String s : strings) {
                p.writeString(s);
            }
        }
    }

    /** SigningDetails: the scheme, signers, keys and lineage. */
    private static void signing(Parcel p, SigningInfo info) {
        signing(p, info == null ? null : info.getSigningDetails());
    }

    private static void signing(Parcel p, SigningDetails details) {
        if (details == null) {
            p.writeInt(-1);
            return;
        }
        p.writeInt(details.getSignatureSchemeVersion());
        signatures(p, details.getSignatures());
        Set<PublicKey> keys = details.getPublicKeys();
        p.writeInt(keys == null ? -1 : keys.size());
        if (keys != null) {
            for (PublicKey key : keys) {
                p.writeSerializable(key);
            }
        }
        signatures(p, details.getPastSigningCertificates());
    }

    private static void signatures(Parcel p, Signature[] signatures) {
        p.writeInt(signatures == null ? -1 : signatures.length);
        if (signatures != null) {
            for (Signature s : signatures) {
                p.writeByteArray(s.toByteArray());
                p.writeInt(s.getFlags());
            }
        }
    }

    /** getInstallSourceInfo, asked in a user the package is installed in; none for null. */
    private static void installSource(Parcel p, PackageState s) {
        InstallSourceInfo info = null;
        SparseArray<? extends PackageUserState> users = s == null ? null : s.getUserStates();
        for (int i = 0; users != null && i < users.size() && info == null; i++) {
            if (!users.valueAt(i).isInstalled()) {
                continue;
            }
            try {
                info = AppGlobals.getPackageManager()
                        .getInstallSourceInfo(s.getPackageName(), users.keyAt(i));
            } catch (RemoteException e) {
                throw e.rethrowFromSystemServer();
            } catch (IllegalArgumentException e) {
                // Not visible in that user after all.
            }
        }
        p.writeBoolean(info != null);
        if (info != null) {
            p.writeString(info.getInstallingPackageName());
            p.writeString(info.getInitiatingPackageName());
            p.writeString(info.getOriginatingPackageName());
            p.writeString(info.getUpdateOwnerPackageName());
            p.writeInt(info.getPackageSource());
            signing(p, info.getInitiatingPackageSigningInfo());
        }
    }

    /** The package's domain verification state; null without web domains. */
    private DomainVerificationInfo domainVerification(String packageName) {
        try {
            return mContext.getSystemService(DomainVerificationManager.class)
                    .getDomainVerificationInfo(packageName);
        } catch (android.content.pm.PackageManager.NameNotFoundException e) {
            return null;
        }
    }

    private static void hostStates(Parcel p, Map<String, Integer> states) {
        p.writeInt(states.size());
        for (Map.Entry<String, Integer> e : new TreeMap<>(states).entrySet()) {
            p.writeString(e.getKey());
            p.writeInt(e.getValue());
        }
    }

    private void userState(Parcel p, PackageUserState u, String packageName, int userId) {
        p.writeLong(u.getCeDataInode());
        p.writeLong(u.getDeDataInode());
        p.writeInt(flags(u.isInstalled(), u.isStopped(), u.isNotLaunched(), u.isHidden(),
                u.isSuspended(), u.isInstantApp(), u.isVirtualPreload(), u.isQuarantined(),
                u.dataExists()));
        p.writeInt(u.getDistractionFlags());
        p.writeInt(u.getEnabledState());
        p.writeString(u.getLastDisableAppCaller());
        Set<String> enabled = u.getEnabledComponents();
        strings(p, enabled == null ? null : new TreeSet<>(enabled));
        Set<String> disabled = u.getDisabledComponents();
        strings(p, disabled == null ? null : new TreeSet<>(disabled));
        p.writeInt(u.getInstallReason());
        p.writeInt(u.getUninstallReason());
        p.writeString(u.getHarmfulAppWarning());
        p.writeString(u.getSplashScreenTheme());
        p.writeLong(u.getFirstInstallTimeMillis());
        p.writeInt(u.getMinAspectRatio());
        ArchiveState archive = u.getArchiveState();
        p.writeBoolean(archive != null);
        if (archive != null) {
            p.writeString(archive.getInstallerTitle());
            p.writeLong(archive.getArchiveTimeMillis());
            List<ArchiveState.ArchiveActivityInfo> activities = archive.getActivityInfos();
            p.writeInt(activities.size());
            for (ArchiveState.ArchiveActivityInfo a : activities) {
                p.writeString(a.getTitle());
                p.writeString(a.getOriginalComponentName().flattenToString());
                p.writeString(path(a.getIconBitmap()));
                p.writeString(path(a.getMonochromeIconBitmap()));
            }
        }
        OverlayPaths overlays = u.getAllOverlayPaths();
        p.writeBoolean(overlays != null);
        if (overlays != null) {
            strings(p, overlays.getOverlayPaths());
            strings(p, overlays.getResourceDirs());
        }
        IPackageManager pm = AppGlobals.getPackageManager();
        try {
            p.writeString(u.isSuspended() ? pm.getSuspendingPackage(packageName, userId) : null);
            p.writeIntArray(u.isInstalled() ? pm.getPackageGids(packageName, 0, userId) : null);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
        TreeSet<String> granted = new TreeSet<>();
        if (u.isInstalled()) {
            Map<String, PermissionManager.PermissionState> states =
                    mContext.createContextAsUser(UserHandle.of(userId), 0)
                            .getSystemService(PermissionManager.class)
                            .getAllPermissionStates(packageName, DEVICE_DEFAULT);
            for (Map.Entry<String, PermissionManager.PermissionState> e : states.entrySet()) {
                if (e.getValue().isGranted()) {
                    granted.add(e.getKey());
                }
            }
        }
        strings(p, granted);
        DomainVerificationUserState selection = domainSelection(packageName, userId);
        p.writeBoolean(selection != null);
        if (selection != null) {
            p.writeBoolean(selection.isLinkHandlingAllowed());
            hostStates(p, selection.getHostToStateMap());
        }
    }

    private static String path(Path path) {
        return path == null ? null : path.toString();
    }

    /** getDomainVerificationUserState in `userId`; null where not visible. */
    private DomainVerificationUserState domainSelection(String packageName, int userId) {
        try {
            return mContext.createContextAsUser(UserHandle.of(userId), 0)
                    .getSystemService(DomainVerificationManager.class)
                    .getDomainVerificationUserState(packageName);
        } catch (android.content.pm.PackageManager.NameNotFoundException e) {
            return null;
        }
    }

    private static byte[] sharedUser(SharedUserApi u) {
        return record(p -> {
            p.writeString(u.getName());
            p.writeInt(u.getAppId());
            p.writeBoolean(u.isPrivileged());
            p.writeInt(u.getSeInfoTargetSdkVersion());
            // In the set's own order, which getPackagesForUid and
            // getNameForUid follow.
            List<String> packages = new ArrayList<>();
            for (PackageState s : u.getPackageStates()) {
                packages.add(s.getPackageName());
            }
            strings(p, packages);
            signing(p, u.getSigningDetails());
        });
    }

    /**
     * A user's record: its preferred activities as the original backs them
     * up (getPreferredActivityBackup, the full XML), or none; and its
     * package-restrictions.xml as the original last wrote it, the only
     * source of its persistent preferred activities and cross-profile
     * intent filters, which no API reads (#715). The file lags the
     * original's memory by its write delay; and its default browser (the
     * browser role's holder).
     */
    private byte[] user(int userId) {
        byte[] preferred;
        try {
            preferred = AppGlobals.getPackageManager().getPreferredActivityBackup(userId);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
        byte[] restrictions = restrictions(userId);
        List<String> browsers = mContext.getSystemService(RoleManager.class)
                .getRoleHoldersAsUser(RoleManager.ROLE_BROWSER, UserHandle.of(userId));
        return record(p -> {
            p.writeInt(userId);
            p.writeByteArray(preferred);
            p.writeByteArray(restrictions);
            p.writeString(browsers.isEmpty() ? null : browsers.get(0));
        });
    }

    /**
     * The user's package-restrictions.xml as ResilientAtomicFile reads it:
     * an interrupted write's backup, else the file, else its reserve copy.
     */
    private static byte[] restrictions(int userId) {
        File dir = Environment.getUserSystemDirectory(userId);
        for (String name : new String[] {"package-restrictions-backup.xml",
                "package-restrictions.xml", "package-restrictions.xml.reservecopy"}) {
            File file = new File(dir, name);
            if (!file.exists()) {
                continue;
            }
            try {
                return Files.readAllBytes(file.toPath());
            } catch (IOException e) {
                Slog.w(TAG, "cannot read " + file, e);
            }
        }
        return null;
    }

    /** ResolverActivity.getLabelRes: ActionTitle's labels, by action (HOME never matches). */
    private static final String[][] RESOLVER_TITLES = {
        {"android.intent.action.VIEW", "whichViewApplicationLabel"},
        {"android.intent.action.EDIT", "whichEditApplicationLabel"},
        {"android.intent.action.SEND", "whichSendApplicationLabel"},
        {"android.intent.action.SENDTO", "whichSendToApplicationLabel"},
        {"android.intent.action.SEND_MULTIPLE", "whichSendApplicationLabel"},
        {"android.media.action.IMAGE_CAPTURE", "whichImageCaptureApplicationLabel"},
        {null, "whichApplicationLabel"},
    };

    /**
     * The system record: the configuration AppsFilter reads from the
     * framework's resources, config_forceSystemPackagesQueryable and
     * config_forceQueryablePackages; what resolveIntent's chooser shows,
     * the resolver activity's theme, its titles and
     * config_customResolverActivity; Settings.Global.DEVICE_PROVISIONED;
     * and the instant app resolver and installer PackageManager chose.
     */
    private byte[] system() {
        Resources res = Resources.getSystem();
        boolean systemQueryable = res.getBoolean(
                res.getIdentifier("config_forceSystemPackagesQueryable", "bool", "android"));
        String[] queryable = res.getStringArray(
                res.getIdentifier("config_forceQueryablePackages", "array", "android"));
        int theme = res.getIdentifier("Theme.Material.Dialog.Alert", "style", "android");
        String customResolver = res.getString(
                res.getIdentifier("config_customResolverActivity", "string", "android"));
        boolean filtering = DeviceConfig.getBoolean(
                DeviceConfig.NAMESPACE_PACKAGE_MANAGER_SERVICE, FILTERING_ENABLED, true);
        int provisioned = Settings.Global.getInt(mContext.getContentResolver(),
                Settings.Global.DEVICE_PROVISIONED, 0);
        ComponentName resolver;
        ComponentName installer;
        try {
            IPackageManager pm = AppGlobals.getPackageManager();
            resolver = pm.getInstantAppResolverComponent();
            installer = pm.getInstantAppInstallerComponent();
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
        return record(p -> {
            p.writeBoolean(systemQueryable);
            strings(p, Arrays.asList(queryable));
            p.writeInt(theme);
            p.writeInt(RESOLVER_TITLES.length);
            for (String[] title : RESOLVER_TITLES) {
                p.writeString(title[0]);
                p.writeInt(res.getIdentifier(title[1], "string", "android"));
            }
            p.writeString(customResolver);
            p.writeInt(provisioned);
            p.writeString(resolver == null ? null : resolver.flattenToString());
            p.writeString(installer == null ? null : installer.flattenToString());
            p.writeBoolean(filtering);
        });
    }

    /** A record's kind and key, in the order the digest takes them. */
    private static final class Key implements Comparable<Key> {
        final int kind;
        final String name;

        Key(int kind, String name) {
            this.kind = kind;
            this.name = name;
        }

        @Override
        public int compareTo(Key other) {
            return kind != other.kind ? Integer.compare(kind, other.kind)
                    : name.compareTo(other.name);
        }

        @Override
        public boolean equals(Object o) {
            return o instanceof Key k && k.kind == kind && k.name.equals(name);
        }

        @Override
        public int hashCode() {
            return kind * 31 + name.hashCode();
        }
    }
}
