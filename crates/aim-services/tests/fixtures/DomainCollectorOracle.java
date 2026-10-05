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
    private DomainCollectorOracle() {}
}
