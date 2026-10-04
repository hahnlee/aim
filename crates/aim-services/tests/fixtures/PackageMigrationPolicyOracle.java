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
                System.out.println("MIGRATION_POLICY_DENIED");
                return;
            }
            System.err.println("MIGRATION_POLICY_REPLY");
            if (!handled) throw new AssertionError("policy transaction was not handled");
            if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID) {
                try { reply.readException(); throw new AssertionError("untrusted policy caller accepted"); }
                catch (SecurityException expected) {}
                verifyDeniedTestBase();
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
    private PackageMigrationPolicyOracle() {}
}
