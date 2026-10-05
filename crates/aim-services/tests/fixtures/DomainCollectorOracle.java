/** Executes the image's collector with explicitly controlled compatibility policy. */
public final class DomainCollectorOracle {
    private static final class Compat extends com.android.server.compat.PlatformCompat {
        boolean restricted;
        @Override public android.os.IBinder asBinder() { return this; }
        Compat(android.content.Context context) { super(context); }
        @Override public boolean isChangeEnabledInternalNoLogging(long id, android.content.pm.ApplicationInfo info) {
            if (id != 175408749L || !"fixture.domains".equals(info.packageName))
                throw new AssertionError("domain compatibility identity differs");
            return restricted;
        }
        @Override public boolean isChangeEnabled(long id, android.content.pm.ApplicationInfo info) { throw new AssertionError("unexpected public compatibility query"); }
        @Override public com.android.internal.compat.CompatibilityChangeConfig getAppConfig(android.content.pm.ApplicationInfo info) { throw new AssertionError("unexpected compatibility config query"); }
    }
    static void verify(java.io.File directory) throws Exception {
        // Own the original mutable nonce memory, as SystemServer does. Its lifetime
        // is this disposable oracle process; later parcel checks also use its cache.
        com.android.internal.os.ApplicationSharedMemory.setInstance(
            com.android.internal.os.ApplicationSharedMemory.create());
        if (android.os.Looper.myLooper() == null) android.os.Looper.prepareMainLooper();
        var context = android.app.ActivityThread.systemMain().getSystemUiContext();
        var compat = new Compat(context);
        verifyConfiguration(directory);
        var config = new com.android.server.SystemConfig(false);
        var collector = new com.android.server.pm.verify.domain.DomainVerificationCollector(compat, config);
        byte[] expected = java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-collector.input").toPath());
        var in = android.os.Parcel.obtain();
        try {
            in.unmarshall(expected, 0, expected.length); in.setDataPosition(0);
            int cases = in.readInt();
            for (int caseId = 0; caseId < cases; caseId++) {
                var pkg = (com.android.server.pm.pkg.AndroidPackage) com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                    java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-collector-" + caseId + ".cache").toPath()));
                for (int restricted = 0; restricted < 2; restricted++) {
                    compat.restricted = restricted != 0;
                    for (int linked = 0; linked < 2; linked++) {
                        config.getLinkedApps().clear();
                        if (linked != 0) config.getLinkedApps().add("fixture.domains");
                        for (int kind = 0; kind < 3; kind++) {
                            var actual = switch (kind) {
                                case 0 -> collector.collectAllWebDomains(pkg);
                                case 1 -> collector.collectValidAutoVerifyDomains(pkg);
                                default -> collector.collectInvalidAutoVerifyDomains(pkg);
                            };
                            String[] ordered = actual.toArray(new String[0]);
                            int count = in.readInt();
                            if (actual.size() != count) throw new AssertionError("collector size case=" + caseId + " restricted=" + restricted + " linked=" + linked + " kind=" + kind + ": " + actual.size() + " != " + count);
                            for (int i = 0; i < count; i++) {
                                String host = in.readString();
                                if (!host.equals(ordered[i])) throw new AssertionError("collector host differs case=" + caseId + " index=" + i);
                            }
                        }
                    }
                }
            }
            if (in.dataAvail() != 0) throw new AssertionError("trailing domain expectations");
        } finally { in.recycle(); }
    }
    private static void verifyConfiguration(java.io.File directory) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-config.input").toPath());
        var in = android.os.Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            for (int sdk : new int[] {27, 28, 36}) {
                var config = new com.android.server.SystemConfig(false);
                String[] paths = {"system/etc/sysconfig", "system/etc/permissions", "vendor/etc/sysconfig", "vendor/etc/sysconfig/sku_demo", "odm/etc/permissions", "odm/etc/permissions/sku_demo", "oem/etc/sysconfig", "product/etc/sysconfig", "product/etc/sysconfig/sku_demo", "system_ext/etc/permissions", "apex/com.fixture/etc/permissions"};
                // Pinned readAllPermissions partition flags, including pre-O-MR1 vendor policy.
                int vendor = 0xc93 | (sdk <= 27 ? 0xc : 0);
                int product = sdk <= 30 ? -1 : 0xfdf;
                int[] flags = {-1, -1, vendor, vendor, vendor, vendor, 0x4a1, product, product, -1, 0x813};
                for (int i = 0; i < paths.length; i++)
                    config.readPermissions(android.util.Xml.newPullParser(), new java.io.File(directory, "domain-config/" + paths[i]), flags[i]);
                String[] actual = config.getLinkedApps().toArray(new String[0]);
                int count = in.readInt();
                if (actual.length != count) throw new AssertionError("linked-app count differs sdk=" + sdk);
                for (int i = 0; i < count; i++)
                    if (!in.readString().equals(actual[i])) throw new AssertionError("linked-app order differs sdk=" + sdk + " index=" + i);
            }
            if (in.dataAvail() != 0) throw new AssertionError("trailing linked-app expectations");
        } finally { in.recycle(); }
    }
    private DomainCollectorOracle() {}
}
