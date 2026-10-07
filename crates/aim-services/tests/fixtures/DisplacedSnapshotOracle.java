package dev.aim.server;

import java.io.File;
import java.nio.file.Files;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

public final class DisplacedSnapshotOracle {
    private static final dev.aim.server.PackageLocal.SigningOwner signing = new dev.aim.server.PackageLocal.SigningOwner() {
        public void add(android.content.pm.SigningDetails oldDetails, android.content.pm.SigningDetails newDetails) {
            android.util.apk.ApkSignatureVerifier.addOverrideSigningDetails(oldDetails, newDetails);
        }
        public void remove(android.content.pm.SigningDetails oldDetails) { android.util.apk.ApkSignatureVerifier.removeOverrideSigningDetails(oldDetails); }
        public void clear() { android.util.apk.ApkSignatureVerifier.clearOverrideSigningDetails(); }
    };
    private static final String INCOMING = "com.google.android.gsf";
    private static final String ORIGINAL = "fixture.original.gsf";
    private static final CurrentLocal currentLocal = new CurrentLocal();
    private static final Permissions permissions = new Permissions();
    private static boolean permissionOwnersRegistered;
    private static final class CurrentLocal implements com.android.server.pm.PackageManagerLocal {
        dev.aim.server.PackageLocal delegate;
        public UnfilteredSnapshot withUnfilteredSnapshot() { return delegate.withUnfilteredSnapshot(); }
        public FilteredSnapshot withFilteredSnapshot() { return delegate.withFilteredSnapshot(); }
        public FilteredSnapshot withFilteredSnapshot(int uid, android.os.UserHandle user) { return delegate.withFilteredSnapshot(uid, user); }
        public void reconcileSdkData(String v, String p, java.util.List<String> d, int u, int a, int o, String s, int f) throws java.io.IOException { delegate.reconcileSdkData(v,p,d,u,a,o,s,f); }
        public void addOverrideSigningDetails(android.content.pm.SigningDetails a, android.content.pm.SigningDetails b) { delegate.addOverrideSigningDetails(a,b); }
        public void removeOverrideSigningDetails(android.content.pm.SigningDetails a) { delegate.removeOverrideSigningDetails(a); }
        public void clearOverrideSigningDetails() { delegate.clearOverrideSigningDetails(); }
    }
    private static final class Permissions implements com.android.server.pm.permission.PermissionManagerServiceInternal {
        int reads;
        int mode;
        Runnable replace;
        public void writeLegacyPermissionStateTEMP() { throw new AssertionError("unused legacy migration owner"); }
        public int[] getGidsForUid(int uid) { throw new AssertionError("unused GID owner"); }
        public com.android.server.pm.permission.LegacyPermissionState getLegacyPermissionState(int id) { throw new AssertionError("unused legacy owner"); }
        public java.util.Set<String> getInstalledPermissions(String name) {
            if (!ORIGINAL.equals(name)) throw new AssertionError("permission definition identity differs");
            return names("fixture.installed");
        }
        public java.util.Set<String> getGrantedPermissions(String name, int user) {
            if (!ORIGINAL.equals(name) || (user != 0 && user != 10)) throw new AssertionError("grant identity differs");
            reads++;
            if (replace != null) { Runnable action = replace; replace = null; action.run(); }
            return names("fixture.granted." + user);
        }
        private java.util.Set<String> names(String prefix) {
            if (mode == 1) return null;
            var names = new java.util.HashSet<String>();
            names.add(prefix + ".z"); names.add(prefix + ".a");
            if (mode == 2) names.add(null);
            return names;
        }
    }
    public static void main(String[] args) {
        try {
            if (args.length > 1) {
                for (boolean grants : new boolean[] {false, true}) {
                    try { permissionCall(grants, 10003, 0); throw new AssertionError("untrusted permission caller accepted"); }
                    catch (SecurityException expected) {}
                }
                System.out.println("permission query owners denied"); System.exit(0);
            }
            for (int i = 0; i < 6; i++) verify(new File(args[0], "case-" + i), i);
            System.out.println("displaced full snapshot contracts: 6 cases");
            System.exit(0);
        } catch (Throwable error) { error.printStackTrace(System.out); System.exit(1); }
    }
    private static void verify(File directory, int index) throws Exception {
        var endpoint = new Owner(directory);
        var visibility = new dev.aim.server.PackageSnapshots.Owner() {
            @Override public String getFilteredPackageName(long version, String candidate, int uid, int user) { throw new UnsupportedOperationException("unfiltered fixture"); }
            @Override public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) { throw new UnsupportedOperationException("unfiltered fixture"); }
        };
        dev.aim.server.PackageSnapshots.Data data;
        try (var lease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(endpoint))) {
            data = lease.captureData(visibility, true);
        }
        try (var snapshot = dev.aim.server.PackageSnapshots.unfiltered(data)) {
            var incoming = snapshot.getPackageStates().get(INCOMING);
            var original = snapshot.getPackageStates().get(ORIGINAL);
            if (incoming == null || original == null || incoming.getAndroidPackage() != null
                    || original.getAndroidPackage() == null || incoming.getSharedUserAppId() != 10002
                    || original.getAppId() != 10003
                    || !INCOMING.equals(((com.android.server.pm.pkg.PackageStateInternal) original).getRealName()))
                throw new AssertionError("displaced identities differ: " + index);
            verifyQueries(new File(directory, "active"), snapshot.getPackageStates());
            verifyQueries(new File(directory, "factory"), snapshot.getDisabledSystemPackageStates());
            // Cases cover independent/shared targets; each has pruned/active/disabled old groups.
            int keep = index % 3;
            boolean shared = index >= 3;
            var oldGroup = snapshot.getSharedUsers().get("fixture.incoming.group");
            if ((oldGroup != null) != (keep != 0)) throw new AssertionError("old group presence differs");
            if (oldGroup != null && (oldGroup.getAppId() != 10002
                    || oldGroup.getPackageStates().contains(incoming)
                    || oldGroup.getPackageStates().size() != (keep == 1 ? 1 : 0)))
                throw new AssertionError("displaced request became a group member");
            if (snapshot.getDisabledSystemPackageStates().size() != (keep == 2 ? 1 : 0))
                throw new AssertionError("factory inventory differs");
            if (shared) {
                var group = snapshot.getSharedUsers().get("fixture.original.group");
                if (group == null || group.getPackageStates().size() != 2
                        || !group.getPackageStates().contains(original)) throw new AssertionError("original retained group differs");
                int retained = 0;
                for (var member : group.getPackageStates()) {
                    if (!ORIGINAL.equals(member.getPackageName())) throw new AssertionError("original member name differs");
                    if (member != original && member.getAndroidPackage() == null) retained++;
                }
                if (retained != 1) throw new AssertionError("original instances collapsed");
            }
        }
        if (endpoint.closes != 1) throw new AssertionError("lease did not close its endpoint");
        endpoint.omitOriginal = true;
        try (var lease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(endpoint))) {
            try { lease.captureData(visibility, true); throw new AssertionError("missing accepted original allowed"); }
            catch (java.io.IOException expected) {}
        }
        verifyStore(directory, new File(directory.getParentFile(), directory.getName() + "-next"), visibility);
    }
    private static void verifyQueries(File directory, java.util.Map<String, ? extends com.android.server.pm.pkg.PackageState> states) throws Exception {
        for (var s : states.values()) {
            File root = new File(directory, s.getPackageName());
            var p = android.os.Parcel.obtain();
            try {
                p.writeString(s.getPackageName()); p.writeInt(s.getAppId()); p.writeString(s.getPath().getPath());
                p.writeString(s.getVolumeUuid()); p.writeString(s.getPrimaryCpuAbi()); p.writeString(s.getSecondaryCpuAbi());
                p.writeString(s.getCpuAbiOverride()); p.writeString(s.getSeInfo()); p.writeString(s.getApexModuleName());
                p.writeLong(s.getVersionCode()); p.writeInt(s.getTargetSdkVersion()); p.writeInt(s.getCategoryOverride());
                p.writeInt(s.getHiddenApiEnforcementPolicy()); p.writeLong(s.getLastModifiedTime()); p.writeLong(s.getLastUpdateTime());
                p.writeByteArray(s.getRestrictUpdateHash());
                for (boolean value : new boolean[] { s.isSystem(),s.isPrivileged(),s.isOem(),s.isVendor(),s.isProduct(),s.isSystemExt(),s.isOdm(),s.isUpdatedSystemApp(),
                        s.isApex(),s.isApkInUpdatedApex(),s.isHiddenUntilInstalled(),s.isDefaultToDeviceProtectedStorage(),s.isForceQueryableOverride(),
                        s.isScannedAsStoppedSystemApp(),s.isUpdateAvailable(),s.isInstallPermissionsFixed(),s.isPendingRestore(),s.isDebuggable(),
                        ((com.android.server.pm.pkg.PackageStateInternal)s).isLoading() }) p.writeInt(value ? 1 : 0);
                s.getSigningInfo().writeToParcel(p, 0);
                compareQuery(new File(root, "query-package"), p.marshall());
            } finally { p.recycle(); }
            for (String id : Files.readAllLines(new File(root, "query-users").toPath())) {
                var u = s.getUserStateOrDefault(Integer.parseInt(id));
                p = android.os.Parcel.obtain();
                try {
                    p.writeLong(u.getCeDataInode()); p.writeLong(u.getDeDataInode());
                    for (boolean value : new boolean[] { u.isInstalled(),u.isStopped(),u.isNotLaunched(),u.isHidden(),u.isInstantApp(),u.isVirtualPreload(),u.isQuarantined(),u.dataExists(),u.isSuspended() }) p.writeInt(value ? 1 : 0);
                    p.writeInt(u.getDistractionFlags()); p.writeInt(u.getEnabledState()); p.writeString(u.getLastDisableAppCaller());
                    queryStrings(p, u.getEnabledComponents() == null ? java.util.List.of() : new java.util.TreeSet<>(u.getEnabledComponents()));
                    queryStrings(p, u.getDisabledComponents() == null ? java.util.List.of() : new java.util.TreeSet<>(u.getDisabledComponents()));
                    p.writeInt(u.getInstallReason()); p.writeInt(u.getUninstallReason()); p.writeString(u.getHarmfulAppWarning());
                    p.writeString(u.getSplashScreenTheme()); p.writeLong(u.getFirstInstallTimeMillis()); p.writeInt(u.getMinAspectRatio());
                    var paths = u.getAllOverlayPaths(); p.writeInt(paths == null ? 0 : 1);
                    if (paths != null) { queryStrings(p, paths.getResourceDirs()); queryStrings(p, paths.getOverlayPaths()); }
                    compareQuery(new File(root, "query-user-" + id), p.marshall());
                } finally { p.recycle(); }
            }
        }
    }
    private static void queryStrings(android.os.Parcel p, java.util.Collection<String> values) {
        p.writeInt(values.size()); for (String value : values) p.writeString(value);
    }
    private static void compareQuery(File expected, byte[] actual) throws Exception {
        byte[] nativeBytes = Files.readAllBytes(expected.toPath());
        if (!Arrays.equals(nativeBytes, actual)) {
            int at = 0; while (at < Math.min(nativeBytes.length, actual.length) && nativeBytes[at] == actual[at]) at++;
            throw new AssertionError("native query projection differs: " + expected + " byte " + at + " lengths " + nativeBytes.length + "/" + actual.length);
        }
    }
    private static void verifyStore(File first, File next, dev.aim.server.PackageSnapshots.Owner visibility) throws Exception {
        Owner[] source = { new Owner(first) };
        var store = new dev.aim.server.PackageSnapshots.Store(
                () -> source[0] == null ? null : dev.aim.server.IPackageScanSnapshot.Stub.asInterface(source[0]), visibility, true);
        try { store.unfiltered(); throw new AssertionError("uninitialized store accepted"); }
        catch (IllegalStateException expected) {}
        long version = source[0].getVersion();
        if (store.refresh() != version || source[0].closes != 1) throw new AssertionError("initial refresh differs");
        verifyLocal(first);
        verifyVersionPage(first);
        verifyComputerLifetime(first, next, visibility);
        verifyPublishedVersion(first, next, visibility);
        var local = new dev.aim.server.PackageLocal(store, (v, p, d, u, a, old, se, f) -> {
            throw new java.io.IOException("unused SDK owner in version fixture");
        }, signing);
        try (var old = local.withUnfilteredSnapshot()) {
            var original = old.getPackageStates().get(ORIGINAL);
            source[0] = new Owner(first);
            if (store.refresh() != version || source[0].closes != 1) throw new AssertionError("same-version lease leaked");
            try (var same = local.withUnfilteredSnapshot()) {
                if (same.getPackageStates().get(ORIGINAL) != original) throw new AssertionError("same-version identity changed");
            }
            source[0] = new Owner(next);
            source[0].omitOriginal = true;
            try { store.refresh(); throw new AssertionError("partial newer graph published"); }
            catch (java.io.IOException expected) {}
            if (store.getVersion() != version || source[0].closes != 1) throw new AssertionError("failed capture changed store");
            source[0] = new Owner(next);
            source[0].failClose = true;
            try { store.refresh(); throw new AssertionError("failed close published graph"); }
            catch (IllegalStateException expected) {}
            if (store.getVersion() != version || source[0].closes != 1) throw new AssertionError("failed close changed store");
            source[0] = new Owner(next);
            if (store.refresh() != version + 1 || source[0].closes != 1) throw new AssertionError("newer retry differs");
            try (var newer = local.withUnfilteredSnapshot()) {
                if (newer.getPackageStates().get(ORIGINAL) == original) throw new AssertionError("different versions reused a replica");
                if (old.getPackageStates().get(ORIGINAL) != original) throw new AssertionError("old scope changed");
            }
            source[0] = new Owner(first);
            try { store.refresh(); throw new AssertionError("backwards version accepted"); }
            catch (java.io.IOException expected) {}
            if (source[0].closes != 1) throw new AssertionError("stale lease leaked");
            source[0] = new Owner(next);
            source[0].failVersion = true;
            source[0].failClose = true;
            try { store.refresh(); throw new AssertionError("failed initial version accepted"); }
            catch (IllegalStateException expected) {
                if (expected.getSuppressed().length != 1) throw new AssertionError("initial close failure lost");
            }
            if (source[0].closes != 1) throw new AssertionError("initial failure lease leaked");
            source[0] = new Owner(next);
            source[0].versionOverride = 0L;
            try { store.refresh(); throw new AssertionError("invalid version accepted"); }
            catch (IllegalStateException expected) {}
            if (source[0].closes != 1) throw new AssertionError("invalid version lease leaked");
            source[0] = null;
            try { store.refresh(); throw new AssertionError("missing endpoint accepted"); }
            catch (java.io.IOException expected) {}
            if (store.getVersion() != version + 1 || old.getPackageStates().get(ORIGINAL) != original)
                throw new AssertionError("rejected refresh replaced state");
        }
    }
    private static byte[] permissionCall(boolean grants, int app, int user) throws Exception {
        var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge"); request.writeString(ORIGINAL);
            if (grants) { request.writeInt(app); request.writeInt(user); }
            int code = grants ? dev.aim.server.IPackageBootstrapBridge.Stub.TRANSACTION_getPackageGrantedPermissions
                    : dev.aim.server.IPackageBootstrapBridge.Stub.TRANSACTION_getPackageInstalledPermissions;
            if (!new dev.aim.server.PackageBootstrapBridge().asBinder().transact(code, request, reply, 0))
                throw new AssertionError("permission transaction unhandled");
            byte[] bytes = reply.marshall(); reply.readException();
            String prefix = grants ? "fixture.granted." + user : "fixture.installed";
            if (!java.util.Arrays.equals(reply.createStringArray(), new String[] {prefix + ".a", prefix + ".z"}) || reply.dataAvail() != 0)
                throw new AssertionError("permission names/order differ");
            return bytes;
        } finally { request.recycle(); reply.recycle(); }
    }
    private static void verifyPermissionOwners(File first, dev.aim.server.PackageLocal local,
            dev.aim.server.PackageLocal replacement) throws Exception {
        if (!permissionOwnersRegistered) {
            try { permissionCall(false, 0, 0); throw new AssertionError("missing permission owner accepted"); }
            catch (IllegalStateException expected) {}
            com.android.server.LocalServices.addService(com.android.server.pm.permission.PermissionManagerServiceInternal.class, permissions);
            try { permissionCall(true, 10003, 0); throw new AssertionError("missing package local owner accepted"); }
            catch (IllegalStateException expected) {}
            com.android.server.LocalManagerRegistry.addManager(com.android.server.pm.PackageManagerLocal.class, currentLocal);
            permissionOwnersRegistered = true;
        }
        currentLocal.delegate = local;
        Files.write(new File(first, "permission-installed.original").toPath(), permissionCall(false, 0, 0));
        for (int user : new int[] {0, 10})
            Files.write(new File(first, "permission-granted-" + user + ".original").toPath(), permissionCall(true, 10003, user));
        int reads = permissions.reads;
        try { permissionCall(true, 10004, 0); throw new AssertionError("foreign permission UID accepted"); }
        catch (IllegalStateException expected) {}
        if (permissions.reads != reads) throw new AssertionError("foreign UID reached permission owner");
        permissions.replace = () -> currentLocal.delegate = replacement;
        try { permissionCall(true, 10003, 0); throw new AssertionError("changed permission owner accepted"); }
        catch (IllegalStateException expected) {}
        currentLocal.delegate = local;
        for (int mode : new int[] {1, 2}) {
            permissions.mode = mode;
            try { permissionCall(false, 0, 0); throw new AssertionError("null definition names accepted"); }
            catch (IllegalStateException expected) {}
            try { permissionCall(true, 10003, 0); throw new AssertionError("null grant names accepted"); }
            catch (IllegalStateException expected) {}
        }
        permissions.mode = 0;
    }
    private static void verifyComputerLifetime(File first, File next,
            dev.aim.server.PackageSnapshots.Owner visibility) throws Exception {
        Owner[] source = { new Owner(first) };
        var store = new dev.aim.server.PackageSnapshots.Store(
                () -> dev.aim.server.IPackageScanSnapshot.Stub.asInterface(source[0]), visibility, true);
        long version = store.refresh();
        var retained = source[0].computer;
        var oldStates = store.unfiltered();
        var computer = store.computer();
        try {
            source[0] = new Owner(next);
            store.refresh();
            if (retained.closes != 0 || computer.getVersion() != version)
                throw new AssertionError("publication closed retained query graph");
            if (computer.getApplicationInfo(ORIGINAL, 0x1234567800000001L, 1010001, 10).uid != version
                    || computer.getPackageInfo(ORIGINAL, 0x1234567800000001L, 1010001, 10).applicationInfo.uid != version
                    || !computer.filterAppAccess(ORIGINAL, 1010001, 10, true)
                    || computer.filterAppAccess(ORIGINAL, 1010001, 10, false))
                throw new AssertionError("internal metadata query used current graph");
            if (computer.getPackageUid(ORIGINAL, 0x1234567800000001L, 10) != version
                    || !java.util.Arrays.equals(computer.getPackagesForUid(1010001),
                            new String[] { ORIGINAL, Long.toString(version) })
                    || !computer.getNameForUid(1010001).equals(Long.toString(version))
                    || !computer.isInstantApp(ORIGINAL, 10)
                    || computer.getTargetSdkVersion(ORIGINAL) != version
                    || !computer.getInstallerPackageName(ORIGINAL, 10).equals(Long.toString(version)))
                throw new AssertionError("expanded query adapter escaped retained version");
            if (computer.getApplicationInfoInternal(ORIGINAL, 0x1234567800000001L, 1010001, 10).uid != version
                    || computer.getPackageInfoInternal(ORIGINAL, 42L, 0x1234567800000001L, 1010001, 10).applicationInfo.uid != version
                    || !computer.isSameApp(ORIGINAL, 0x1234567800000001L, 1010001, 10)
                    || !computer.isSameApp(ORIGINAL, 1010001, 10)
                    || computer.isSameApp(null, 1010001, 10)
                    || !computer.filterAppAccess(1010001, 10002)
                    || !computer.canQueryPackage(1010001, "fixture.query.target")
                    || !computer.canQueryPackage(1010001, null))
                throw new AssertionError("internal caller identity or versioned metadata changed");
            var capturedStates = oldStates.getPackageStates();
            var original = (com.android.server.pm.pkg.PackageStateInternal) capturedStates.get(ORIGINAL);
            if (computer.getPackageUidInternal(ORIGINAL, 0x1234567800000001L, 10) != version
                    || computer.getPackageTargetSdkVersion(ORIGINAL) != original.getPkg().getTargetSdkVersion()
                    || computer.getPackageTargetSdkVersion("fixture.missing") != android.os.Build.VERSION_CODES.CUR_DEVELOPMENT
                    || computer.isPackageEphemeral(10, ORIGINAL) != original.getUserStateOrDefault(10).isInstantApp()
                    || computer.isPackageEphemeral(10, "fixture.missing")
                    || computer.getAndroidPackage("fixture.normalized") != original.getAndroidPackage()
                    || computer.getPackage(ORIGINAL) != original.getAndroidPackage()
                    || computer.getPackage("fixture.missing") != null)
                throw new AssertionError("internal read adapter used public query semantics");
            if (computer.getPackageStateInternal("fixture.normalized") != original
                    || computer.getPackageStateInternal("fixture.explicit", 1010001) != original
                    || computer.getPackageStateFiltered("fixture.normalized", 1010001, 10) != original
                    || computer.getPackageStateFiltered("fixture.hidden", 1010001, 10) != null
                    || computer.getUidTargetSdkVersion(1010001) != version
                    || computer.getDisabledSystemPackage("fixture.missing") != null)
                throw new AssertionError("normalized/filtered/UID state adapter changed");
            for (var entry : oldStates.getDisabledSystemPackageStates().entrySet()) {
                if (computer.getDisabledSystemPackage(entry.getKey()) != entry.getValue())
                    throw new AssertionError("disabled setting replica identity changed");
            }
            var internalStates = computer.getPackageStates();
            if (!internalStates.equals(capturedStates)) throw new AssertionError("internal state inventory changed");
            var visitedStates = new java.util.ArrayList<com.android.server.pm.pkg.PackageStateInternal>();
            computer.forEachPackageState(visitedStates::add);
            if (!visitedStates.equals(new java.util.ArrayList<>(internalStates.values())))
                throw new AssertionError("internal package iteration order changed");
            var visitedPackages = new java.util.ArrayList<com.android.server.pm.pkg.AndroidPackage>();
            computer.forEachPackage(visitedPackages::add);
            var expectedPackages = new java.util.ArrayList<com.android.server.pm.pkg.AndroidPackage>();
            for (var state : internalStates.values()) if (state.getPkg() != null) expectedPackages.add(state.getPkg());
            if (!visitedPackages.equals(expectedPackages)) throw new AssertionError("internal parsed package iteration changed");
            internalStates.clear();
            if (computer.getPackageStates().size() != capturedStates.size())
                throw new AssertionError("caller changed captured inventory");
            if (computer.getSharedUserApi(-1) != null || !computer.getSharedUserPackages(-1).isEmpty())
                throw new AssertionError("unknown shared UID returned owner");
            for (var group : oldStates.getSharedUsers().values()) {
                if (computer.getSharedUserApi(group.getAppId()) != group
                        || !computer.getSharedUserPackages(group.getAppId()).equals(group.getPackageStates()))
                    throw new AssertionError("shared UID replica identity changed");
            }
            for (var state : capturedStates.values()) {
                if (state.hasSharedUser()) continue;
                try { computer.getSharedUserApi(state.getAppId()); throw new AssertionError("package UID treated as shared"); }
                catch (ClassCastException expected) {}
                try { computer.getSharedUserPackages(state.getAppId()); throw new AssertionError("package UID returned shared members"); }
                catch (ClassCastException expected) {}
            }
            store.close();
            if (source[0].computer.closes != 1 || retained.closes != 0)
                throw new AssertionError("store close escaped capture ownership");
            computer.close();
            Runnable[] closedQueries = {
                () -> computer.getPackageUid(ORIGINAL, 0x1234567800000001L, 10),
                () -> computer.getPackagesForUid(1010001),
                () -> computer.getNameForUid(1010001),
                () -> computer.isInstantApp(ORIGINAL, 10),
                () -> computer.getTargetSdkVersion(ORIGINAL),
                () -> computer.getInstallerPackageName(ORIGINAL, 10),
                () -> computer.filterAppAccess(ORIGINAL, 1010001, 10, true),
                () -> computer.getApplicationInfoInternal(ORIGINAL, 0x1234567800000001L, 1010001, 10),
                () -> computer.getPackageInfoInternal(ORIGINAL, 42L, 0x1234567800000001L, 1010001, 10),
                () -> computer.isSameApp(ORIGINAL, 1010001, 10),
                () -> computer.filterAppAccess(1010001, 10002),
                () -> computer.canQueryPackage(1010001, "fixture.query.target"),
                () -> computer.getPackageUidInternal(ORIGINAL, 0x1234567800000001L, 10),
                () -> computer.isPackageEphemeral(10, ORIGINAL),
                () -> computer.getPackageTargetSdkVersion(ORIGINAL),
                () -> computer.getAndroidPackage("fixture.normalized"),
                () -> computer.getPackage(ORIGINAL),
                () -> computer.getPackageStates(),
                () -> computer.forEachPackageState(value -> {}),
                () -> computer.forEachPackage(value -> {}),
                () -> computer.getSharedUserApi(-1),
                () -> computer.getSharedUserPackages(-1),
                () -> computer.getPackageStateInternal("fixture.explicit", 1010001),
                () -> computer.getPackageStateFiltered("fixture.normalized", 1010001, 10),
                () -> computer.getUidTargetSdkVersion(1010001),
                () -> computer.getDisabledSystemPackage("fixture.missing"),
            };
            for (Runnable query : closedQueries) {
                try { query.run(); throw new AssertionError("closed expanded query adapter accepted"); }
                catch (IllegalStateException expected) {}
            }
            if (retained.closes != 0) throw new AssertionError("query close invalidated retained state scope");
            oldStates.close();
            if (retained.closes != 1) throw new AssertionError("last scope leaked query endpoint");
            try { computer.getVersion(); throw new AssertionError("closed computer adapter accepted"); }
            catch (IllegalStateException expected) {}
            try { store.computer(); throw new AssertionError("closed store accepted query scope"); }
            catch (IllegalStateException expected) {}
        } finally {
            computer.close(); oldStates.close(); store.close();
        }
    }
    private static void verifyVersionPage(File directory) throws Exception {
        var path = Files.createTempFile(new File("/data/local/tmp/package-version-oracle").toPath(), "version-page-", ".bin");
        try (var writer = new java.io.RandomAccessFile(path.toFile(), "rw")) {
            writer.setLength(Long.BYTES);
            writer.write(java.nio.ByteBuffer.allocate(Long.BYTES)
                    .order(java.nio.ByteOrder.LITTLE_ENDIAN).putLong(31).array());
            writer.getFD().sync();
            var descriptor = android.os.ParcelFileDescriptor.open(path.toFile(),
                    android.os.ParcelFileDescriptor.MODE_READ_ONLY);
            var page = new dev.aim.server.PackageVersionPage(descriptor);
            try (page) {
                if (descriptor.getFileDescriptor().valid())
                    throw new AssertionError("mapped page retained descriptor ownership");
                if (page.currentVersion() != 31) throw new AssertionError("initial mapped version differs");
                writer.seek(0);
                writer.write(java.nio.ByteBuffer.allocate(Long.BYTES)
                        .order(java.nio.ByteOrder.LITTLE_ENDIAN).putLong(32).array());
                writer.getFD().sync();
                if (page.currentVersion() != 32) throw new AssertionError("mapped page missed publication");
                Files.delete(path);
                if (page.currentVersion() != 32) throw new AssertionError("unlinked version page lost mapping");
            }
            try { page.currentVersion(); throw new AssertionError("closed version page accepted"); }
            catch (IllegalStateException expected) {}
        } finally {
            Files.deleteIfExists(path);
        }
    }
    private static void verifyPublishedVersion(File first, File next,
            dev.aim.server.PackageSnapshots.Owner visibility) throws Exception {
        Owner[] source = { new Owner(first) };
        long[] published = { source[0].getVersion() };
        int[] captures = { 0 };
        var store = new dev.aim.server.PackageSnapshots.Store(() -> {
            captures[0]++;
            return dev.aim.server.IPackageScanSnapshot.Stub.asInterface(source[0]);
        }, visibility, true, () -> published[0]);
        store.refresh();
        try (var old = store.unfiltered()) {
            var original = old.getPackageStates().get(ORIGINAL);
            try (var same = store.unfiltered()) {
                if (same.getPackageStates().get(ORIGINAL) != original || captures[0] != 1)
                    throw new AssertionError("unchanged published version recaptured graph");
            }
            published[0]++;
            try { store.unfiltered(); throw new AssertionError("stale capture accepted after publication"); }
            catch (IllegalStateException expected) {}
            source[0] = new Owner(next);
            source[0].omitOriginal = true;
            try { store.unfiltered(); throw new AssertionError("failed automatic refresh returned stale data"); }
            catch (IllegalStateException expected) {}
            source[0] = new Owner(next);
            try (var newer = store.unfiltered()) {
                if (newer.getPackageStates().get(ORIGINAL) == original)
                    throw new AssertionError("published version did not replace replica");
                if (old.getPackageStates().get(ORIGINAL) != original)
                    throw new AssertionError("automatic refresh changed retained snapshot");
            }
            int count = captures[0];
            if (store.getVersion() != published[0] || captures[0] != count)
                throw new AssertionError("unchanged version issued capture");
            published[0]--;
            try { store.unfiltered(); throw new AssertionError("backwards published version accepted"); }
            catch (IllegalStateException expected) {}
        }
    }
    private static void verifyLocal(File first) throws Exception {
        int[] caller = { -1, -1 };
        var visibility = new dev.aim.server.PackageSnapshots.Owner() {
            public String getFilteredPackageName(long version, String name, int uid, int user) {
                caller[0] = uid; caller[1] = user; return name;
            }
            public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) {
                caller[0] = uid; caller[1] = user; return false;
            }
        };
        var store = new dev.aim.server.PackageSnapshots.Store(
                () -> dev.aim.server.IPackageScanSnapshot.Stub.asInterface(new Owner(first)), visibility, true);
        Object[][] observed = { null };
        java.io.IOException failure = new java.io.IOException("fixture SDK data failure");
        boolean[] fail = { false };
        dev.aim.server.PackageLocal.SdkDataOwner sdk = (volume, name, dirs, user, app, oldApp, seinfo, flags) -> {
            observed[0] = new Object[] { volume, name, dirs, user, app, oldApp, seinfo, flags };
            if (fail[0]) throw failure;
        };
        try { new dev.aim.server.PackageLocal(store, sdk, signing); throw new AssertionError("uninitialized facade accepted"); }
        catch (IllegalStateException expected) {}
        store.refresh();
        var local = new dev.aim.server.PackageLocal(store, sdk, signing);
        try { new dev.aim.server.PackageLocal(store, null, signing); throw new AssertionError("missing SDK owner accepted"); }
        catch (NullPointerException expected) {}
        try { new dev.aim.server.PackageLocal(store, sdk, null); throw new AssertionError("missing signing owner accepted"); }
        catch (NullPointerException expected) {}
        try (var scope = local.withFilteredSnapshot(1010001, android.os.UserHandle.of(10))) {
            if (scope.getPackageState(ORIGINAL) == null || caller[0] != 1010001 || caller[1] != 10)
                throw new AssertionError("explicit local caller differs");
        }
        try (var scope = local.withFilteredSnapshot()) {
            if (scope.getPackageState(ORIGINAL) == null
                    || caller[0] != android.os.Binder.getCallingUid()
                    || caller[1] != android.os.Binder.getCallingUserHandle().getIdentifier())
                throw new AssertionError("Binder local caller differs");
        }
        var other = new dev.aim.server.PackageSnapshots.Store(
                () -> dev.aim.server.IPackageScanSnapshot.Stub.asInterface(new Owner(first)), visibility, true);
        other.refresh();
        verifyPermissionOwners(first, local, new dev.aim.server.PackageLocal(other, sdk, signing));
        try (var old = local.withUnfilteredSnapshot(); var alternate = other.unfiltered()) {
            var uncommitted = alternate.getPackageStates().get(ORIGINAL);
            if (old.getPackageStates().get(ORIGINAL) == uncommitted) throw new AssertionError("fixture reused replica");
            try (var scope = local.withFilteredSnapshot(uncommitted)) {
                if (scope.getPackageState(ORIGINAL) != uncommitted
                        || scope.getPackageStates().get(ORIGINAL) != uncommitted)
                    throw new AssertionError("uncommitted local scope differs");
            }
        }
        var dirs = java.util.List.of("sdk-a", "sdk-b");
        local.reconcileSdkData(null, INCOMING, dirs, 10, 10042, 10041, "platform:targetSdkVersion=36", 3);
        if (!java.util.Arrays.equals(observed[0], new Object[] { null, INCOMING, dirs, 10, 10042, 10041,
                "platform:targetSdkVersion=36", 3 }) || observed[0][2] != dirs)
            throw new AssertionError("SDK arguments changed");
        fail[0] = true;
        try { local.reconcileSdkData("volume", INCOMING, dirs, 0, 10042, -1, "default", 1);
            throw new AssertionError("SDK failure swallowed"); }
        catch (java.io.IOException expected) {
            if (expected != failure) throw new AssertionError("SDK failure replaced");
        }
        var unknown = android.content.pm.SigningDetails.UNKNOWN;
        Runnable[] overrides = {
            () -> local.addOverrideSigningDetails(unknown, unknown),
            () -> local.removeOverrideSigningDetails(unknown),
            () -> local.clearOverrideSigningDetails(),
        };
        for (var operation : overrides) {
            try {
                operation.run();
                if (!android.os.Build.isDebuggable()) throw new AssertionError("release signing override accepted");
            } catch (SecurityException expected) {
                if (android.os.Build.isDebuggable()
                        || !"This test API is only available on debuggable builds".equals(expected.getMessage()))
                    throw expected;
            }
        }
    }
    private static final class CapturedComputer extends dev.aim.server.IPackageComputer.Stub {
        final long version;
        int closes;
        CapturedComputer(long version) { this.version = version; }
        void open() { if (closes != 0) throw new IllegalStateException("fixture computer closed"); }
        public long getVersion() { open(); return version; }
        public android.content.pm.ApplicationInfo getApplicationInfo(String n, long flags, int user,
                int filter, int caller, int pid) {
            open();
            if (!ORIGINAL.equals(n) || flags != 0x1234567800000001L || user != 10 || filter != 1010001
                    || caller != android.os.Binder.getCallingUid() || pid != android.os.Binder.getCallingPid())
                throw new AssertionError("internal query caller or flags changed");
            var info = new android.content.pm.ApplicationInfo();
            info.uid = (int) version;
            return info;
        }
        public android.content.pm.PackageInfo getPackageInfo(String n, long flags, int user,
                int filter, int caller, int pid) {
            var info = new android.content.pm.PackageInfo();
            info.applicationInfo = getApplicationInfo(n, flags, user, filter, caller, pid);
            return info;
        }
        public boolean filterAppAccess(String n, int caller, int user, boolean filterUninstalled) {
            open();
            if (!ORIGINAL.equals(n) || caller != 1010001 || user != 10)
                throw new AssertionError("visibility query caller changed");
            return filterUninstalled;
        }
        private void caller(int uid, int pid) {
            open();
            if (uid != android.os.Binder.getCallingUid() || pid != android.os.Binder.getCallingPid())
                throw new AssertionError("actual internal query caller changed");
        }
        private void packageUser(String name, int user) {
            if (!ORIGINAL.equals(name) || user != 10) throw new AssertionError("internal package identity changed");
        }
        public int getPackageUid(String name, long flags, int user, int uid, int pid) {
            caller(uid, pid); packageUser(name, user);
            if (flags != 0x1234567800000001L) throw new AssertionError("internal UID flags truncated");
            return (int) version;
        }
        public String[] getPackagesForUid(int target, int uid, int pid) {
            caller(uid, pid);
            if (target != 1010001) throw new AssertionError("target UID changed");
            return new String[] { ORIGINAL, Long.toString(version) };
        }
        public String getNameForUid(int target, int uid, int pid) {
            caller(uid, pid);
            if (target != 1010001) throw new AssertionError("target UID changed");
            return Long.toString(version);
        }
        public boolean isInstantApp(String name, int user, int uid, int pid) {
            caller(uid, pid); packageUser(name, user); return true;
        }
        public int getTargetSdkVersion(String name, int uid, int pid) {
            caller(uid, pid);
            if (!ORIGINAL.equals(name)) throw new AssertionError("target SDK identity changed");
            return (int) version;
        }
        public String getInstallerPackageName(String name, int user, int uid, int pid) {
            caller(uid, pid); packageUser(name, user); return Long.toString(version);
        }
        public int getPackageUidInternal(String name, long flags, int userId) {
            open(); packageUser(name, userId);
            if (flags != 0x1234567800000001L) throw new AssertionError("internal system UID flags changed");
            return (int) version;
        }
        public String resolveInternalPackageName(String name, long versionCode, int callerUid) {
            open();
            int expectedCaller = "fixture.explicit".equals(name) ? 1010001 : android.os.Binder.getCallingUid();
            if (versionCode != android.content.pm.PackageManager.VERSION_CODE_HIGHEST || callerUid != expectedCaller)
                throw new AssertionError("package normalization caller changed");
            return "fixture.normalized".equals(name) || "fixture.explicit".equals(name) ? ORIGINAL : name;
        }
        public boolean isSameApp(String name, long flags, int comparisonUid, int userId,
                int callingUid, int callingPid) {
            caller(callingUid, callingPid);
            if (comparisonUid != 1010001 || userId != 10
                    || (flags != 0 && flags != 0x1234567800000001L))
                throw new AssertionError("same-app caller or flags changed");
            return ORIGINAL.equals(name);
        }
        public boolean filterUidAccess(int targetUid, int callingUid) {
            open();
            if (targetUid != 1010001 || callingUid != 10002)
                throw new AssertionError("UID filter caller changed");
            return true;
        }
        public boolean canQueryPackage(int queryUid, String target, int callingUid, int callingPid) {
            caller(callingUid, callingPid);
            if (queryUid != 1010001 || (target != null && !target.equals("fixture.query.target")))
                throw new AssertionError("query package identity changed");
            return true;
        }
        public android.content.pm.PackageInfo getPackageInfoInternal(String name, long versionCode,
                long flags, int userId, int filterUid, int callingUid, int callingPid) {
            if (versionCode != 42L) throw new AssertionError("package metadata version code changed");
            return getPackageInfo(name, flags, userId, filterUid, callingUid, callingPid);
        }
        public String getPackageStateFilteredName(String name, int caller, int user) {
            open();
            if (caller != 1010001 || user != 10) throw new AssertionError("filtered state caller changed");
            if ("fixture.hidden".equals(name)) return null;
            if (!"fixture.normalized".equals(name)) throw new AssertionError("filtered state name changed");
            return ORIGINAL;
        }
        public int getUidTargetSdkVersion(int uid) {
            open();
            if (uid != 1010001) throw new AssertionError("UID SDK target changed");
            return (int) version;
        }
        public void close() {
            if (++closes != 1) throw new AssertionError("query capture closed twice");
        }
    }
    private static final class Owner extends dev.aim.server.IPackageScanSnapshot.Stub {
        private final File directory;
        boolean omitOriginal;
        boolean failVersion;
        boolean failClose;
        Long versionOverride;
        int closes;
        Owner(File directory) { this.directory = directory; }
        private byte[] read(String path) {
            var file = new File(directory, path);
            if (!file.exists()) return null;
            try { return Files.readAllBytes(file.toPath()); }
            catch (java.io.IOException error) { throw new java.io.UncheckedIOException(error); }
        }
        private String scope(String name, boolean factory) { return (factory ? "factory/" : "active/") + name + "/"; }
        private String[] lines(String path) { return new String(java.util.Objects.requireNonNull(read(path)), StandardCharsets.UTF_8).lines().toArray(String[]::new); }
        private int length(String path) { byte[] bytes = read(path); return bytes == null ? -1 : bytes.length; }
        private byte[] chunk(String path, int offset, int length) { return Arrays.copyOfRange(java.util.Objects.requireNonNull(read(path)), offset, offset + length); }
        @Override public android.os.IInterface queryLocalInterface(String descriptor) { return null; }
        CapturedComputer computer;
        public dev.aim.server.IPackageComputer getComputer() {
            computer = new CapturedComputer(getVersion());
            return computer;
        }
        @Override public long getVersion() {
            if (failVersion) throw new IllegalStateException("fixture version failure");
            return versionOverride == null ? Long.parseLong(lines("version")[0]) : versionOverride;
        }
        @Override public String[] getPackageNames(boolean factory) {
            return Arrays.stream(lines(factory ? "factory-names" : "active-names")).filter(n -> factory || !omitOriginal || !n.equals(ORIGINAL)).toArray(String[]::new);
        }
        @Override public int getCodeLength(String n, boolean f) { return length(scope(n,f)+"code"); }
        @Override public byte[] getCodeChunk(String n, boolean f, int o, int l) { return chunk(scope(n,f)+"code",o,l); }
        @Override public int getRuntimeStateLength(String n, boolean f) { return length(scope(n,f)+"runtime"); }
        @Override public byte[] getRuntimeStateChunk(String n, boolean f, int o, int l) { return chunk(scope(n,f)+"runtime",o,l); }
        @Override public int getSettingLength(String n, boolean f) { return length(scope(n,f)+"setting"); }
        @Override public byte[] getSettingChunk(String n, boolean f, int o, int l) { return chunk(scope(n,f)+"setting",o,l); }
        @Override public byte[] getSigningState(String n, boolean f) { return read(scope(n,f)+"signing"); }
        @Override public byte[] getTransientState(String n, boolean f) { return read(scope(n,f)+"transient"); }
        @Override public int getHiddenApiEnforcementPolicy(String n, boolean f) { return Integer.parseInt(lines(scope(n,f)+"hidden")[0]); }
        @Override public int[] getUserStateIds(String n, boolean f) { return Arrays.stream(lines(scope(n,f)+"users")).mapToInt(Integer::parseInt).toArray(); }
        @Override public int getUserStateLength(String n, boolean f, int u) { return length(scope(n,f)+"user-"+u); }
        @Override public byte[] getUserStateChunk(String n, boolean f, int u, int o, int l) { return chunk(scope(n,f)+"user-"+u,o,l); }
        @Override public String[] getSharedUserNames() { return lines("groups"); }
        @Override public int getSharedUserStateLength(String n) { return length("group/"+n); }
        @Override public byte[] getSharedUserStateChunk(String n, int o, int l) { return chunk("group/"+n,o,l); }
        @Override public byte[] getUsage(String n) { throw new UnsupportedOperationException("scoped runtime owns usage"); }
        @Override public byte[] getSeInfo(String n) { throw new UnsupportedOperationException("scoped runtime owns seInfo"); }
        @Override public int getLibraryStateLength(String n) { throw new UnsupportedOperationException("scoped runtime owns libraries"); }
        @Override public byte[] getLibraryStateChunk(String n, int o, int l) { throw new UnsupportedOperationException("scoped runtime owns libraries"); }
        @Override public void close() { closes++; if (failClose) throw new IllegalStateException("fixture close failure"); }
    }
}
