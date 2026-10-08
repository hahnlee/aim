package dev.aim.server;

public final class PackageMigrationPolicyOracle {
    public static void main(String[] args) throws Exception {
        System.err.println("MIGRATION_POLICY_MAIN");
        var request = android.os.Parcel.obtain();
        var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
            boolean handled;
            try {
                System.err.println("MIGRATION_POLICY_TRANSACTION");
                handled = new PackageBootstrapBridge().asBinder().transact(
                    IPackageBootstrapBridge.Stub.TRANSACTION_isSharedUidMigrationBestEffort,
                    request, reply, 0);
            } catch (SecurityException denied) {
                if (android.os.Binder.getCallingUid() == android.os.Process.SYSTEM_UID) throw denied;
                verifyDeniedTestBase();
                verifyDeniedQueryOwners();
                System.out.println("MIGRATION_POLICY_DENIED");
                return;
            }
            System.err.println("MIGRATION_POLICY_REPLY");
            if (!handled) throw new AssertionError("policy transaction was not handled");
            if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID) {
                try { reply.readException(); throw new AssertionError("untrusted policy caller accepted"); }
                catch (SecurityException expected) {}
                verifyDeniedTestBase();
                verifyDeniedQueryOwners();
                System.out.println("MIGRATION_POLICY_DENIED");
            } else {
                System.err.println("MIGRATION_POLICY_MARSHALL");
                byte[] bytes = reply.marshall();
                System.err.println("MIGRATION_POLICY_EXCEPTION");
                reply.readException();
                System.err.println("MIGRATION_POLICY_BOOLEAN");
                boolean bestEffort = reply.readBoolean();
                System.err.println("MIGRATION_POLICY_COMPARE");
                if (reply.dataAvail() != 0 || bestEffort != com.android.server.pm.SharedUidMigration.applyStrategy(2))
                    throw new AssertionError("original policy decision differs");
                System.err.println("MIGRATION_POLICY_WRITE");
                java.nio.file.Files.write(new java.io.File(args[0], "migration-policy.original").toPath(), bytes);
                verifyTestBase(new java.io.File(args[0]));
                verifyQueryOwners(new java.io.File(args[0]));
                verifyDomainPolicy(new java.io.File(args[0]));
                com.android.server.pm.ScanSettingsWriteOracle.verifyCurrentVersion(new java.io.File(args[0]), currentVersionPayload());
                verifyCacheInvalidation(new java.io.File(args[0]));
                verifyVerifierIdentity(new java.io.File(args[0]));
                System.out.println("MIGRATION_POLICY " + (bestEffort ? 1 : 0));
            }
        } finally { request.recycle(); reply.recycle(); }
        System.exit(0);
    }
    private static void verifyTestBase(java.io.File directory) throws Exception {
        var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
            com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(new java.io.File(directory, "test-base.cache").toPath()));
        var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
            android.os.ServiceManager.getService("platform_compat"));
        if (compat == null) throw new AssertionError("original compatibility owner absent");
        for (int sdk : new int[] {29, 30}) {
            pkg.setTargetSdkVersion(sdk);
            var appInfo = com.android.server.pm.parsing.pkg.AndroidPackageUtils.generateAppInfoWithoutState(pkg);
            if (appInfo.isSystemApp() || appInfo.uid != -1)
                throw new AssertionError("test-base input is not pre-registration non-system metadata");
            var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
            try {
                request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
                request.writeByteArray(com.android.server.pm.parsing.PackageCacher.toCacheEntryStatic(pkg));
                if (!new PackageBootstrapBridge().asBinder().transact(
                        IPackageBootstrapBridge.Stub.TRANSACTION_isTestBaseLibraryChangeEnabled, request, reply, 0))
                    throw new AssertionError("test-base transaction unhandled");
                byte[] bytes = reply.marshall(); reply.readException();
                boolean enabled = reply.readBoolean();
                if (reply.dataAvail() != 0 || enabled != compat.isChangeEnabled(133396946L, appInfo))
                    throw new AssertionError("original test-base policy differs");
                java.nio.file.Files.write(new java.io.File(directory, "test-base-" + sdk + ".original").toPath(), bytes);
            } finally { request.recycle(); reply.recycle(); }
        }
    }
    private static void verifyQueryOwners(java.io.File directory) throws Exception {
        var bridge = new PackageBootstrapBridge().asBinder();
        var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                android.os.ServiceManager.getService("platform_compat"));
        if (compat == null) throw new AssertionError("query compatibility owner absent");
        for (int sdk : new int[] {28, 29, 30, 36}) {
            var info = new android.content.pm.ApplicationInfo();
            info.packageName = "fixture.query.compat"; info.targetSdkVersion = sdk;
            var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
            try {
                request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
                request.writeString(info.packageName); request.writeInt(sdk);
                if (!bridge.transact(IPackageBootstrapBridge.Stub.TRANSACTION_isApplicationQueryFilteringEnabled, request, reply, 0))
                    throw new AssertionError("query compatibility transaction unhandled");
                byte[] bytes = reply.marshall(); reply.readException();
                if (reply.readBoolean() != compat.getAppConfig(info).isChangeEnabled(135549675L) || reply.dataAvail() != 0)
                    throw new AssertionError("query compatibility owner differs");
                java.nio.file.Files.write(new java.io.File(directory, "query-compat-" + sdk + ".original").toPath(), bytes);
            } finally { request.recycle(); reply.recycle(); }
        }
        // This client has no SystemServer-local permission owner.
        var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge"); request.writeInt(1000);
            boolean denied = false;
            try {
                bridge.transact(IPackageBootstrapBridge.Stub.TRANSACTION_getPermissionGidsForUid, request, reply, 0);
                reply.readException();
            } catch (IllegalStateException expected) { denied = true; }
            if (!denied) throw new AssertionError("missing local permission owner accepted");
        } finally { request.recycle(); reply.recycle(); }
    }
    private static final class LocalCompat extends com.android.server.compat.PlatformCompat {
        @Override public boolean isChangeEnabledByUid(long id, int uid) { throw new AssertionError("unexpected UID compatibility query"); }
        LocalCompat(android.content.Context context) { super(context); }
        @Override public android.os.IBinder asBinder() { return this; }
        @Override public boolean isChangeEnabled(long id, android.content.pm.ApplicationInfo info) { throw new AssertionError("unexpected logging compatibility query"); }
        @Override public com.android.internal.compat.CompatibilityChangeConfig getAppConfig(android.content.pm.ApplicationInfo info) { throw new AssertionError("unexpected local config query"); }
    }
    private static void verifyDomainPolicy(java.io.File directory) throws Exception {
        var uuidRequest = android.os.Parcel.obtain(); var uuidReply = android.os.Parcel.obtain();
        try {
            uuidRequest.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
            if (!new PackageBootstrapBridge().asBinder().transact(IPackageBootstrapBridge.Stub.TRANSACTION_isDomainSetUuidStrictValidationEnabled, uuidRequest, uuidReply, 0)) throw new AssertionError("UUID mode transaction unhandled");
            uuidReply.readException();
            boolean expected = dalvik.system.VMRuntime.getSdkVersion() >= 34 && android.compat.Compatibility.isChangeEnabled(263076149L);
            if (uuidReply.readBoolean() != expected || uuidReply.dataAvail() != 0) throw new AssertionError("original UUID mode differs");
        } finally { uuidRequest.recycle(); uuidReply.recycle(); }
        com.android.internal.os.ApplicationSharedMemory.setInstance(com.android.internal.os.ApplicationSharedMemory.create());
        if (android.os.Looper.myLooper() == null) android.os.Looper.prepareMainLooper();
        var original = new LocalCompat(android.app.ActivityThread.systemMain().getSystemUiContext());
        var bridge = new PackageBootstrapBridge().asBinder();
        int[] codes = {IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerificationRestricted,
                IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerificationSettingsV2Enabled};
        long[] changes = {175408749L, 178111421L};
        for (int kind = 0; kind < codes.length; kind++) {
            for (int sdk : new int[] {28, 30, 31, 36}) {
                var info = new android.content.pm.ApplicationInfo();
                info.packageName = "fixture.domains.compat"; info.targetSdkVersion = sdk;
                var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
                try {
                    request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge"); request.writeString(info.packageName); request.writeInt(sdk);
                    if (!bridge.transact(codes[kind], request, reply, 0)) throw new AssertionError("domain policy transaction unhandled");
                    byte[] bytes = reply.marshall(); reply.readException();
                    if (reply.readBoolean() != original.isChangeEnabledInternalNoLogging(changes[kind], info) || reply.dataAvail() != 0)
                        throw new AssertionError("original domain policy differs change=" + changes[kind] + " sdk=" + sdk);
                    String prefix = kind == 0 ? "domain-compat-" : "domain-settings-v2-";
                    java.nio.file.Files.write(new java.io.File(directory, prefix + sdk + ".original").toPath(), bytes);
                } finally { request.recycle(); reply.recycle(); }
            }
            invalidDomainPolicyInput(bridge, codes[kind], null, 31);
            invalidDomainPolicyInput(bridge, codes[kind], "", 31);
            invalidDomainPolicyInput(bridge, codes[kind], "fixture.domains.compat", -1);
        }
    }
    private static void invalidDomainPolicyInput(android.os.IBinder bridge, int code, String name, int sdk) throws Exception {
        var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge"); request.writeString(name); request.writeInt(sdk);
            try {
                if (!bridge.transact(code, request, reply, 0)) throw new AssertionError("invalid domain input transaction unhandled");
                reply.readException();
            } catch (IllegalArgumentException expected) { return; }
            throw new AssertionError("invalid domain compatibility input accepted");
        } finally { request.recycle(); reply.recycle(); }
    }
    private static final class CacheProbe extends android.app.PropertyInvalidatedCache<String, Integer> {
        int count;
        CacheProbe() { super(4, "cache_key.system_server.package_info_cache"); }
        @Override public Integer recompute(String query) { return ++count; }
    }
    private static void verifyCacheInvalidation(java.io.File directory) throws Exception {
        var cache = new CacheProbe(); cache.invalidateCache();
        int before = cache.query("probe");
        if (cache.query("probe") != before) throw new AssertionError("original cache did not retain an entry");
        var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
            if (!new PackageBootstrapBridge().asBinder().transact(IPackageBootstrapBridge.Stub.TRANSACTION_invalidatePackageInfoCache, request, reply, 0))
                throw new AssertionError("cache invalidation transaction unhandled");
            byte[] bytes = reply.marshall(); reply.readException();
            if (reply.dataAvail() != 0 || cache.query("probe") == before) throw new AssertionError("package-info cache entry survived invalidation");
            java.nio.file.Files.write(new java.io.File(directory, "package-cache-invalidation.original").toPath(), bytes);
        } finally { request.recycle(); reply.recycle(); }
    }
    private static final class VerifierProxy implements com.android.server.pm.verify.domain.proxy.DomainVerificationProxy {
        public android.content.ComponentName getComponentName() { throw new AssertionError("unused verifier component owner"); }
        public boolean isCallerVerifier(int uid) { return uid == 10073 || uid == 1010073; }
        public void sendBroadcastForPackages(java.util.Set<String> names) { throw new AssertionError("identity broadcast"); }
        public boolean runMessage(int code, Object object) { throw new AssertionError("identity message"); }
    }
    private static final class DomainIdentityOwner implements com.android.server.pm.verify.domain.DomainVerificationManagerInternal {
        final com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy;
        DomainIdentityOwner(com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy) { this.proxy = proxy; }
        public java.util.UUID generateNewId() { throw new AssertionError("identity UUID generation"); }
        public com.android.server.pm.verify.domain.proxy.DomainVerificationProxy getProxy() { return proxy; }
    }
    private static boolean verifierReply(android.os.IBinder bridge, int uid) throws Exception {
        var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge"); request.writeInt(uid);
            if (!bridge.transact(IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerifierUid, request, reply, 0)) throw new AssertionError("identity transaction unhandled");
            reply.readException(); boolean selected = reply.readBoolean();
            if (reply.dataAvail() != 0) throw new AssertionError("trailing identity reply"); return selected;
        } finally { request.recycle(); reply.recycle(); }
    }
    private static void verifyVerifierIdentity(java.io.File directory) throws Exception {
        var proxy = new VerifierProxy();
        var bridge = new PackageBootstrapBridge(new DomainIdentityOwner(proxy)).asBinder();
        for (int uid : new int[] {0, 1000, 2000, 10001, 10073, 1010073, 1010074})
            if (verifierReply(bridge, uid) != proxy.isCallerVerifier(uid)) throw new AssertionError("proxy UID delegation differs");
        var original = new com.android.server.pm.verify.domain.DomainVerificationService(null, new com.android.server.SystemConfig(false), null);
        bridge = new PackageBootstrapBridge(original).asBinder();
        if (verifierReply(bridge, 10073) != original.getProxy().isCallerVerifier(10073)) throw new AssertionError("original unavailable proxy differs");
        try { verifierReply(new PackageBootstrapBridge().asBinder(), 10073); throw new AssertionError("missing identity owner accepted"); }
        catch (IllegalStateException expected) {}
        try { verifierReply(new PackageBootstrapBridge(new DomainIdentityOwner(null)).asBinder(), 10073); throw new AssertionError("missing proxy accepted"); }
        catch (IllegalStateException expected) {}
        try { verifierReply(bridge, -1); throw new AssertionError("negative identity UID accepted"); }
        catch (IllegalArgumentException expected) {}
    }
    private static void verifyDeniedQueryOwners() throws Exception {
        for (int code : new int[] {IPackageBootstrapBridge.Stub.TRANSACTION_isApplicationQueryFilteringEnabled,
                IPackageBootstrapBridge.Stub.TRANSACTION_getPermissionGidsForUid,
                IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerificationRestricted,
                IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerificationSettingsV2Enabled,
                IPackageBootstrapBridge.Stub.TRANSACTION_invalidatePackageInfoCache,
                IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerifierUid,
                IPackageBootstrapBridge.Stub.TRANSACTION_isDomainSetUuidStrictValidationEnabled,
                IPackageBootstrapBridge.Stub.TRANSACTION_getCurrentPackageVersion}) {
            var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
            try {
                request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
                if (code == IPackageBootstrapBridge.Stub.TRANSACTION_invalidatePackageInfoCache || code == IPackageBootstrapBridge.Stub.TRANSACTION_isDomainSetUuidStrictValidationEnabled || code == IPackageBootstrapBridge.Stub.TRANSACTION_getCurrentPackageVersion) {
                    // No arguments.
                } else if (code == IPackageBootstrapBridge.Stub.TRANSACTION_isDomainVerifierUid) {
                    request.writeInt(10073);
                } else if (code != IPackageBootstrapBridge.Stub.TRANSACTION_getPermissionGidsForUid) {
                    request.writeString("fixture.query.compat"); request.writeInt(30);
                } else { request.writeInt(1000); }
                try {
                    if (!new PackageBootstrapBridge().asBinder().transact(code, request, reply, 0))
                        throw new AssertionError("query owner denial transaction unhandled");
                    reply.readException();
                } catch (SecurityException expected) { continue; }
                throw new AssertionError("untrusted query owner caller accepted");
            } finally { request.recycle(); reply.recycle(); }
        }
    }
    private static void verifyDeniedTestBase() throws Exception {
        var request = android.os.Parcel.obtain(); var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge"); request.writeByteArray(null);
            try {
                if (!new PackageBootstrapBridge().asBinder().transact(
                        IPackageBootstrapBridge.Stub.TRANSACTION_isTestBaseLibraryChangeEnabled, request, reply, 0))
                    throw new AssertionError("test-base denial transaction unhandled");
                reply.readException();
            } catch (SecurityException expected) { return; }
            throw new AssertionError("untrusted test-base caller accepted");
        } finally { request.recycle(); reply.recycle(); }
    }
    public static byte[] currentVersionPayload() throws Exception {
        var request=android.os.Parcel.obtain(); var reply=android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
            if(!new PackageBootstrapBridge().asBinder().transact(IPackageBootstrapBridge.Stub.TRANSACTION_getCurrentPackageVersion,request,reply,0)) throw new AssertionError("build-version transaction unhandled");
            reply.readException(); var bytes=reply.createByteArray();
            if(bytes==null || reply.dataAvail()!=0) throw new AssertionError("build-version reply malformed");
            return bytes;
        } finally { request.recycle(); reply.recycle(); }
    }
    private PackageMigrationPolicyOracle() {}
}
