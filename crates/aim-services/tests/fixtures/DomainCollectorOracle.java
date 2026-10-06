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
        verifySignatures(directory);
        verifyAttachment(directory, context, compat);
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
    private static void verifySignatures(java.io.File directory) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-signatures.input").toPath());
        var in = android.os.Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            int cases = in.readInt();
            for (int i = 0; i < cases; i++) {
                var signatures = new android.content.pm.Signature[in.readInt()];
                for (int j = 0; j < signatures.length; j++)
                    signatures[j] = new android.content.pm.Signature(java.util.HexFormat.of().parseHex(in.readString()));
                String expected = in.readString();
                if (!expected.equals(android.util.PackageUtils.computeSignaturesSha256Digest(signatures)))
                    throw new AssertionError("domain backup signature digest differs case=" + i);
            }
            if (in.dataAvail() != 0) throw new AssertionError("trailing domain signature expectations");
        } finally { in.recycle(); }
    }
    private static void verifyAttachment(java.io.File directory, android.content.Context context, Compat compat) throws Exception {
        compat.restricted = true;
        var code = (com.android.server.pm.pkg.AndroidPackage) com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
            java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-owner.cache").toPath()));
        for (int i = 0; i < 4; i++) {
            var config = new com.android.server.SystemConfig(false);
            if (i == 1) config.getLinkedApps().add("fixture.domains");
            var service = new com.android.server.pm.verify.domain.DomainVerificationService(context, config, compat);
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-owner-" + i + ".input"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next();
                service.readSettings(null, parser);
            }
            var oldSetting = domainSetting(code, i == 1, "00000000-0000-0000-0000-00000000000b");
            service.addPackage((com.android.server.pm.pkg.PackageStateInternal) oldSetting, null);
            writeDomains(directory, i, "add", service);
            writeQueries(directory, i, "add", service, oldSetting);
            var newSetting = domainSetting(code, i == 1, "00000000-0000-0000-0000-00000000000c");
            service.migrateState((com.android.server.pm.pkg.PackageStateInternal) oldSetting, (com.android.server.pm.pkg.PackageStateInternal) newSetting, null);
            writeDomains(directory, i, "migrate", service);
            writeQueries(directory, i, "migrate", service, newSetting);
        }
    }
    private static com.android.server.pm.PackageSetting domainSetting(com.android.server.pm.pkg.AndroidPackage code, boolean system, String id) {
        var setting = new com.android.server.pm.PackageSetting("fixture.domains", null, new java.io.File("/data/app/fixture.domains"), system ? 1 : 0, 0, java.util.UUID.fromString(id));
        setting.setPkg(code);
        setting.setSigningDetails(new android.content.pm.SigningDetails(new android.content.pm.Signature[0], 0, new android.util.ArraySet<>(), null));
        return setting;
    }
    private static void writeDomains(java.io.File directory, int caseId, String stage, com.android.server.pm.verify.domain.DomainVerificationService service) throws Exception {
        try (var output = new java.io.FileOutputStream(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".original"))) {
            var xml = android.util.Xml.resolveSerializer(output);
            xml.startDocument(null, true); xml.startTag(null, "packages");
            service.writeSettings(null, xml, false, -1);
            xml.endTag(null, "packages"); xml.endDocument();
        }
    }
    private static final class DomainComputer implements com.android.server.pm.Computer {
        final com.android.server.pm.pkg.PackageStateInternal setting;
        DomainComputer(com.android.server.pm.PackageSetting setting) { this.setting = (com.android.server.pm.pkg.PackageStateInternal) setting; }
        public com.android.server.pm.pkg.PackageStateInternal getPackageStateInternal(String name) {
            if (!"fixture.domains".equals(name)) throw new AssertionError("foreign domain query package");
            return setting;
        }
    }
    private static final class DomainConnection implements com.android.server.pm.verify.domain.DomainVerificationManagerInternal.Connection {
        final DomainComputer computer;
        int user;
        DomainConnection(com.android.server.pm.PackageSetting setting) { computer = new DomainComputer(setting); }
        public int getCallingUid() { return 1000; }
        public int getCallingUserId() { return user; }
        public int[] getAllUserIds() { return new int[] {0, 10}; }
        public com.android.server.pm.Computer snapshot() { return computer; }
        public boolean doesUserExist(int id) { return id == 0 || id == 10; }
        public boolean filterAppAccess(String name, int uid, int id) {
            if (!"fixture.domains".equals(name) || uid != 1000 || !doesUserExist(id)) throw new AssertionError("foreign domain visibility identity");
            return false;
        }
        public void scheduleWriteSettings() { throw new AssertionError("unexpected query write"); }
    }
    private static void writeStates(android.os.Parcel out, java.util.Map<String, Integer> states) {
        var sorted = new java.util.TreeMap<>(states);
        out.writeInt(sorted.size());
        for (var entry : sorted.entrySet()) { out.writeString(entry.getKey()); out.writeInt(entry.getValue()); }
    }
    private static void writeQueries(java.io.File directory, int caseId, String stage, com.android.server.pm.verify.domain.DomainVerificationService service, com.android.server.pm.PackageSetting setting) throws Exception {
        var connection = new DomainConnection(setting); service.setConnection(connection);
        var out = android.os.Parcel.obtain();
        try {
            var info = service.getDomainVerificationInfo("fixture.domains");
            out.writeInt(info == null ? 0 : 1);
            if (info != null) { out.writeString(info.getIdentifier().toString()); writeStates(out, info.getHostToStateMap()); }
            for (int user : new int[] {0, 10}) {
                connection.user = user;
                var selection = service.getDomainVerificationUserState("fixture.domains", user);
                if (selection == null) throw new AssertionError("missing attached user state");
                out.writeInt(selection.isLinkHandlingAllowed() ? 1 : 0);
                writeStates(out, selection.getHostToStateMap());
            }
            java.nio.file.Files.write(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".queries").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    private DomainCollectorOracle() {}
}
