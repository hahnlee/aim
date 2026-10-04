package dev.aim.server;

public final class PackageMigrationPolicyOracle {
    public static void main(String[] args) throws Exception {
        var request = android.os.Parcel.obtain();
        var reply = android.os.Parcel.obtain();
        try {
            request.writeInterfaceToken("dev.aim.server.IPackageBootstrapBridge");
            boolean handled;
            try {
                handled = new PackageBootstrapBridge().asBinder().transact(
                    IPackageBootstrapBridge.Stub.TRANSACTION_isSharedUidMigrationBestEffort,
                    request, reply, 0);
            } catch (SecurityException denied) {
                if (android.os.Binder.getCallingUid() == android.os.Process.SYSTEM_UID) throw denied;
                System.out.println("MIGRATION_POLICY_DENIED");
                return;
            }
            if (!handled) throw new AssertionError("policy transaction was not handled");
            if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID) {
                try { reply.readException(); throw new AssertionError("untrusted policy caller accepted"); }
                catch (SecurityException expected) {}
                System.out.println("MIGRATION_POLICY_DENIED");
            } else {
                byte[] bytes = reply.marshall();
                reply.readException();
                boolean bestEffort = reply.readBoolean();
                if (reply.dataAvail() != 0 || bestEffort != com.android.server.pm.SharedUidMigration.applyStrategy(2))
                    throw new AssertionError("original policy decision differs");
                java.nio.file.Files.write(new java.io.File(args[0], "migration-policy.original").toPath(), bytes);
                System.out.println("MIGRATION_POLICY " + (bestEffort ? 1 : 0));
            }
        } finally { request.recycle(); reply.recycle(); }
        System.exit(0);
    }
    private PackageMigrationPolicyOracle() {}
}
