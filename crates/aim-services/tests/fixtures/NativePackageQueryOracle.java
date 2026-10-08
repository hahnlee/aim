package dev.aim.server;

/** Actual native Snapshot/Computer capabilities; no replacement framework owner. */
public final class NativePackageQueryOracle {
    private static final class Owners implements PackageSnapshots.Source, PackageSnapshots.Owner, AutoCloseable {
        private final IServiceHost host;
        private final java.util.Map<Long, IPackageComputer> filters = new java.util.HashMap<>();
        private final java.util.Map<Long, java.util.Set<String>> active = new java.util.HashMap<>();
        Owners(IServiceHost host) { this.host = java.util.Objects.requireNonNull(host); }
        public IPackageScanSnapshot capture() throws android.os.RemoteException {
            var scan = java.util.Objects.requireNonNull(host.capturePackageScan());
            android.os.Binder.allowBlocking(scan.asBinder());
            long version = scan.getVersion();
            if (!filters.containsKey(version)) {
                var computer = java.util.Objects.requireNonNull(scan.getComputer());
                android.os.Binder.allowBlocking(computer.asBinder());
                if (computer.getVersion() != version) throw new AssertionError("native filter capture version differs");
                filters.put(version, computer);
                active.put(version, java.util.Set.copyOf(java.util.Arrays.asList(scan.getPackageNames(false))));
            }
            return scan;
        }
        private IPackageComputer filter(long version) {
            return java.util.Objects.requireNonNull(filters.get(version), "uncaptured filter version");
        }
        public String getFilteredPackageName(long version, String name, int uid, int user) {
            try { return filter(version).getPackageStateFilteredName(name, uid, user); }
            catch (android.os.RemoteException error) { throw error.rethrowFromSystemServer(); }
        }
        public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) {
            if (!active.get(version).contains(state.getPackageName())) throw new AssertionError("candidate outside actual active capture");
            try { return filter(version).shouldFilterApplication(0, null, state.getPackageName(), state.getAppId(),
                    uid, user, false, android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
            catch (android.os.RemoteException error) { throw error.rethrowFromSystemServer(); }
        }
        public void close() throws android.os.RemoteException {
            for (var computer : filters.values()) computer.close();
            filters.clear(); active.clear();
        }
    }
    public static void main(String[] args) throws Throwable {
        try { run(args); }
        catch (Throwable failure) {
            failure.printStackTrace(System.out);
            System.out.flush();
            throw failure;
        }
    }
    private static void run(String[] args) throws Exception {
        var host = IServiceHost.Stub.asInterface(android.os.Binder.allowBlocking(java.util.Objects.requireNonNull(
                android.os.ServiceManager.checkService(args.length > 1 ? args[1] : "host"))));
        try (var owners = new Owners(host)) {
            if (args.length > 0 && args[0].equals("user-delta")) {
                var manager = android.content.pm.IPackageManager.Stub.asInterface(android.os.Binder.allowBlocking(
                        java.util.Objects.requireNonNull(android.os.ServiceManager.checkService(args.length > 2 ? args[2] : "query_package"))));
                try (var versions = new PackageVersionPage(java.util.Objects.requireNonNull(host.getPackageStateVersionPage()))) {
                    PackageSnapshotUserDeltaOracle.verify(owners, owners, versions, manager,
                            args.length > 3 ? args[3] : "android");
                }
                System.out.println("ORIGINAL_NATIVE_USER_DELTA stopped component expired isolation");
                return;
            }
            var intent = new android.content.Intent("android.intent.action.PACKAGE_CHANGED");
            PackageServiceQueryOracle.verify(owners, owners, intent, null, 0, 0, 1000, 0, false);
            PackageReceiverQueryOracle.verify(owners, owners, intent, null, 0, 0, 1000, 0, false);
            PackagePureQueryOracle.verify(owners, owners, "android", 0, 1000, 0, 1000, false);
            var internal = IPackageInternalHost.Stub.asInterface(android.os.Binder.allowBlocking(
                    java.util.Objects.requireNonNull(host.getPackageInternalHost())));
            try (var versions = new PackageVersionPage(java.util.Objects.requireNonNull(host.getPackageStateVersionPage()));
                    var store = new PackageSnapshots.Store(owners, owners, true, versions)) {
                store.refresh();
                PackageSnapshotUsageOracle.verify(store, () -> {
                    try { internal.notifyPackageUse("android", 0,
                            android.os.Binder.getCallingUid(), android.os.Binder.getCallingPid()); }
                    catch (android.os.RemoteException error) { throw error.rethrowFromSystemServer(); }
                }, "android", 0);
            }
        }
        System.out.println("ORIGINAL_NATIVE_QUERY service receiver visibility application parity usage publication\n");
    }
}
