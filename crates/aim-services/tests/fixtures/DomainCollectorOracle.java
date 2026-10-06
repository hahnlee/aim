/** Executes the image's collector with explicitly controlled compatibility policy. */
public final class DomainCollectorOracle {
    private static final class Compat extends com.android.server.compat.PlatformCompat {
        boolean restricted;
        boolean v2 = true;
        @Override public android.os.IBinder asBinder() { return this; }
        Compat(android.content.Context context) { super(context); }
        @Override public boolean isChangeEnabledInternalNoLogging(long id, android.content.pm.ApplicationInfo info) {
            if (id == 178111421L) return v2;
            if (id != 175408749L || !("fixture.domains".equals(info.packageName) || "competitor".equals(info.packageName) || info.packageName.startsWith("fixture.owner.")))
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
        DomainEnforcerOracle.verify(directory);
        verifySignatures(directory);
        DomainOwnerSortOracle.write(directory);
        verifyAttachment(directory, context, compat);
        verifyGroupedOwners(directory, context, compat);
        verifyPersistenceDefaults(directory, context, compat);
        verifyNativeWrites(directory, context, compat);
        com.android.server.pm.ScanSettingsWriteOracle.verifyLegacyDomains(directory, context, compat);
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
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-cleanup-legacy.input"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readLegacySettings(parser);
            }
            var oldSetting = domainSetting(code, i == 1, "00000000-0000-0000-0000-00000000000b");
            service.addPackage((com.android.server.pm.pkg.PackageStateInternal) oldSetting, null);
            writeDomains(directory, i, "add", service);
            writeQueries(directory, i, "add", service, oldSetting);
            writeApprovals(directory, i, "add", service, oldSetting, compat);
            var newSetting = domainSetting(code, i == 1, "00000000-0000-0000-0000-00000000000c");
            service.migrateState((com.android.server.pm.pkg.PackageStateInternal) oldSetting, (com.android.server.pm.pkg.PackageStateInternal) newSetting, null);
            writeDomains(directory, i, "migrate", service);
            writeQueries(directory, i, "migrate", service, newSetting);
            writeApprovals(directory, i, "migrate", service, newSetting, compat);
            var competitor = (com.android.internal.pm.parsing.pkg.PackageImpl) com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-owner.cache").toPath()));
            competitor.setPackageName("competitor");
            var competitorSetting = new com.android.server.pm.PackageSetting("competitor", null, new java.io.File("/data/app/competitor"), 0, 0, java.util.UUID.fromString("00000000-0000-0000-0000-000000000012"));
            competitorSetting.setPkg((com.android.server.pm.pkg.AndroidPackage) competitor);
            service.addPackage((com.android.server.pm.pkg.PackageStateInternal) competitorSetting, null);
            var connection = new DomainConnection(newSetting); service.setConnection(connection);
            String[] groupHosts = {"h0.example", "h1.example", "*.wild.example", "undeclared.example", "-edge.example", "numeric.1", "bad_name.example", "δοκιμή.example", "x", "*." + "a".repeat(64) + ".example"};
            var update = new android.os.Bundle();
            for (String host : groupHosts) update.putParcelableArrayList(host, new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(1, "/first")))));
            service.setUriRelativeFilterGroups("fixture.domains", update); writeDomains(directory, i, "uri-add", service);
            update = new android.os.Bundle();
            update.putParcelableArrayList("h0.example", new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(0, "/second")))));
            update.putParcelableArrayList("h1.example", new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>());
            update.putParcelableArrayList("*.wild.example", null);
            service.setUriRelativeFilterGroups("fixture.domains", update); writeDomains(directory, i, "uri-update", service);
            service.setUriRelativeFilterGroups("missing", new android.os.Bundle());
            try { service.setUriRelativeFilterGroups("missing", update); throw new AssertionError("missing URI owner accepted"); }
            catch (android.content.pm.PackageManager.NameNotFoundException expected) {}
            if (connection.writes != 0) throw new AssertionError("URI group update scheduled an unexpected settings write");
            writeUriGroups(directory, i, service, java.util.Arrays.asList(groupHosts));

            var hosts = new java.util.TreeSet<String>(); hosts.add("h0.example");
            if (service.setDomainVerificationStatus(java.util.UUID.fromString("00000000-0000-0000-0000-000000000000"), hosts, 1) != 1) throw new AssertionError("invalid domain UUID status differs");
            hosts.add("unknown.example");
            if (service.setDomainVerificationStatus(java.util.UUID.fromString("00000000-0000-0000-0000-00000000000c"), hosts, 1) != 2 || !hosts.equals(java.util.Set.of("h0.example"))) throw new AssertionError("unknown domain filtering differs");
            try { service.setDomainVerificationStatus(java.util.UUID.fromString("00000000-0000-0000-0000-00000000000c"), new java.util.TreeSet<>(), 1); throw new AssertionError("empty domains accepted"); }
            catch (IllegalArgumentException expected) {}
            try { service.setDomainVerificationStatus(java.util.UUID.fromString("00000000-0000-0000-0000-00000000000c"), hosts, 0); throw new AssertionError("invalid verifier state accepted"); }
            catch (IllegalArgumentException expected) {}
            if (connection.writes != 0) throw new AssertionError("failed verification scheduled persistence");
            hosts.clear(); for (int h = 0; h <= 8; h++) hosts.add("h" + h + ".example"); hosts.add("h1024.example");
            for (int state : new int[] {1, 1024}) {
                if (service.setDomainVerificationStatus(java.util.UUID.fromString("00000000-0000-0000-0000-00000000000c"), hosts, state) != 0) throw new AssertionError("verifier update failed");
                writeDomains(directory, i, "verified-" + state, service);
            }

            service.setDomainVerificationLinkHandlingAllowedInternal("fixture.domains", false, 0); writeDomains(directory, i, "link-single", service);
            service.setDomainVerificationLinkHandlingAllowedInternal("fixture.domains", true, -1); writeDomains(directory, i, "link-all-users", service);
            service.setDomainVerificationLinkHandlingAllowedInternal(null, false, 11); writeDomains(directory, i, "link-all-packages", service);
            try { service.setDomainVerificationLinkHandlingAllowedInternal("missing", false, 0); throw new AssertionError("missing link owner accepted"); }
            catch (android.content.pm.PackageManager.NameNotFoundException expected) {}

            service.clearPackageForUser("fixture.domains", 10); writeDomains(directory, i, "package-user", service);
            service.clearUser(10); writeDomains(directory, i, "user", service);
            service.clearPackage("fixture.domains"); writeDomains(directory, i, "package", service);
            service.clearPackage("pending.only"); service.clearPackage("restored.only");
            writeDomains(directory, i, "pending-restored", service);
            service.clearPackage("missing"); service.clearPackageForUser("missing", 10); service.clearUser(-1);
            if (connection.writes != 13) throw new AssertionError("domain cleanup persistence requests differ: " + connection.writes);

        }
    }
    private static android.content.UriRelativeFilterGroup uriGroup(int action, String path) {
        var group = new android.content.UriRelativeFilterGroup(action);
        group.addUriRelativeFilter(new android.content.UriRelativeFilter(0, 0, path));
        group.addUriRelativeFilter(new android.content.UriRelativeFilter(1, 0, "q=1"));
        group.addUriRelativeFilter(new android.content.UriRelativeFilter(2, 1, "fragment"));
        group.addUriRelativeFilter(new android.content.UriRelativeFilter(0, 1, "😀"));
        group.addUriRelativeFilter(new android.content.UriRelativeFilter(0, 0, "Aa"));
        group.addUriRelativeFilter(new android.content.UriRelativeFilter(0, 0, "BB"));
        return group;
    }
    private static void writeUriGroups(java.io.File directory, int caseId, com.android.server.pm.verify.domain.DomainVerificationService service, java.util.List<String> hosts) throws Exception {
        var out = android.os.Parcel.obtain();
        try {
            var wire = android.os.Parcel.obtain();
            try {
                var requests = new java.util.ArrayList<>(hosts); requests.add(null); requests.add(hosts.get(0));
                java.util.List<java.util.List<String>> lists = java.util.Arrays.asList(requests, java.util.Collections.emptyList(), null);
                for (String name : new String[] {"fixture.domains", "missing", null}) for (var list : lists) {
                    try { var bundle = service.getUriRelativeFilterGroups(name, list); wire.writeNoException(); wire.writeTypedObject(bundle, 0); }
                    catch (NullPointerException error) { wire.writeException(error); }
                }
                java.nio.file.Files.write(new java.io.File(directory, "domain-uri-" + caseId + ".reply").toPath(), wire.marshall());
            } finally { wire.recycle(); }
            var result = service.getUriRelativeFilterGroups("fixture.domains", hosts);
            var values = new java.util.TreeMap<String, java.util.List<android.content.UriRelativeFilterGroup>>();
            for (String host : hosts) {
                var parcels = result.getParcelableArrayList(host, android.content.UriRelativeFilterGroupParcel.class);
                if (parcels != null) values.put(host, android.content.UriRelativeFilterGroup.parcelsToGroups(parcels));
            }
            out.writeInt(values.size());
            for (var entry : values.entrySet()) {
                out.writeString(entry.getKey()); out.writeInt(entry.getValue().size());
                for (var group : entry.getValue()) {
                    out.writeInt(group.getAction()); out.writeInt(group.getUriRelativeFilters().size());
                    for (var filter : group.getUriRelativeFilters()) { out.writeInt(filter.getUriPart()); out.writeInt(filter.getPatternType()); out.writeString(filter.getFilter()); }
                }
            }
            java.nio.file.Files.write(new java.io.File(directory, "domain-uri-" + caseId + ".original").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    private static com.android.server.pm.PackageSetting domainSetting(com.android.server.pm.pkg.AndroidPackage code, boolean system, String id) {
        var setting = new com.android.server.pm.PackageSetting(code.getPackageName(), null, new java.io.File("/data/app/" + code.getPackageName()), system ? 1 : 0, 0, java.util.UUID.fromString(id));
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
    private static void verifyPersistenceDefaults(java.io.File directory, android.content.Context context, Compat compat) throws Exception {
        for (int i = 0; i < 5; i++) {
            var service = new com.android.server.pm.verify.domain.DomainVerificationService(context, new com.android.server.SystemConfig(false), compat);
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-default-" + i + ".input"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readSettings(null, parser);
            }
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-default-" + i + ".legacy"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readLegacySettings(parser);
            }
            try (var output = new java.io.FileOutputStream(new java.io.File(directory, "domain-default-" + i + ".original"))) {
                var xml = android.util.Xml.resolveSerializer(output); xml.startDocument(null, true); xml.startTag(null, "packages");
                service.writeSettings(null, xml, false, -1); xml.endTag(null, "packages"); xml.endDocument();
            }
        }
    }
    private static void verifyNativeWrites(java.io.File directory, android.content.Context context, Compat compat) throws Exception {
        for (int i = 0; i < 5; i++) {
            var service = new com.android.server.pm.verify.domain.DomainVerificationService(context, new com.android.server.SystemConfig(false), compat);
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-written-" + i + ".input"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readSettings(null, parser);
            }
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-written-" + i + ".legacy"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readLegacySettings(parser);
            }
            try (var output = new java.io.FileOutputStream(new java.io.File(directory, "domain-written-" + i + ".original"))) {
                var xml = android.util.Xml.resolveSerializer(output); xml.startDocument(null, true); xml.startTag(null, "packages");
                service.writeSettings(null, xml, false, -1); xml.endTag(null, "packages"); xml.endDocument();
            }
        }
    }
    private static final class DomainComputer implements com.android.server.pm.Computer {
        final java.util.Map<String, com.android.server.pm.pkg.PackageStateInternal> settings;
        DomainComputer(com.android.server.pm.PackageSetting setting) { this(java.util.Map.of("fixture.domains", (com.android.server.pm.pkg.PackageStateInternal) setting)); }
        DomainComputer(java.util.Map<String, com.android.server.pm.pkg.PackageStateInternal> settings) { this.settings = settings; }
        public com.android.server.pm.pkg.PackageStateInternal getPackageStateInternal(String name) { return settings.get(name); }
    }
    private static final class DomainConnection implements com.android.server.pm.verify.domain.DomainVerificationManagerInternal.Connection {
        final DomainComputer computer;
        int user;
        int writes;
        DomainConnection(com.android.server.pm.PackageSetting setting) { computer = new DomainComputer(setting); }
        DomainConnection(java.util.Map<String, com.android.server.pm.pkg.PackageStateInternal> settings) { computer = new DomainComputer(settings); }
        public int getCallingUid() { return 1000; }
        public int getCallingUserId() { return user; }
        public int[] getAllUserIds() { return new int[] {0, 10}; }
        public com.android.server.pm.Computer snapshot() { return computer; }
        public boolean doesUserExist(int id) { return id == 0 || id == 10; }
        public boolean filterAppAccess(String name, int uid, int id) {
            if (!"fixture.domains".equals(name) || uid != 1000 || !doesUserExist(id)) throw new AssertionError("foreign domain visibility identity");
            return false;
        }
        public void scheduleWriteSettings() { writes++; }
    }
    private static void writeApprovals(java.io.File directory, int caseId, String stage,
            com.android.server.pm.verify.domain.DomainVerificationService service,
            com.android.server.pm.PackageSetting setting, Compat compat) throws Exception {
        service.setConnection(new DomainConnection(setting));
        var out = android.os.Parcel.obtain();
        var ownersOut = android.os.Parcel.obtain();
        try {
            for (boolean v2 : new boolean[] {false, true}) {
                compat.v2 = v2;
                for (int mode = -1; mode <= 8; mode++) {
                    for (String host : new String[] {"h0.example", "h1.example", "h4.example", "h7.example", "h8.example", "h1024.example", "example", "sub.example", "notexample", "unknown.invalid"}) {
                        out.writeInt(com.android.server.pm.DomainApprovalFixture.approval(service, setting, mode, host));
                        var owners = service.getOwnersForDomain(host, 0);
                        ownersOut.writeNoException(); ownersOut.writeInt(owners.size());
                        for (var owner : owners) ownersOut.writeTypedObject(owner, 0);
                    }
                }
            }
            java.nio.file.Files.write(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".approvals").toPath(), out.marshall());
            java.nio.file.Files.write(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".owners").toPath(), ownersOut.marshall());
        } finally { compat.v2 = true; out.recycle(); ownersOut.recycle(); }
    }
    private static void verifyGroupedOwners(java.io.File directory, android.content.Context context, Compat compat) throws Exception {
        var service = new com.android.server.pm.verify.domain.DomainVerificationService(context, new com.android.server.SystemConfig(false), compat);
        for (String file : new String[] {"domain-owners-group.input", "domain-owners-group.legacy"}) {
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, file))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next();
                if (file.endsWith(".legacy")) service.readLegacySettings(parser); else service.readSettings(null, parser);
            }
        }
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-owners-group.users").toPath());
        var in = android.os.Parcel.obtain(); var out = android.os.Parcel.obtain();
        var settings = new java.util.HashMap<String, com.android.server.pm.pkg.PackageStateInternal>();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            int count = in.readInt();
            for (int i = 0; i < count; i++) {
                String name = in.readString(); String id = in.readString(); int level = in.readInt(); long time = in.readLong();
                var code = (com.android.internal.pm.parsing.pkg.PackageImpl) com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                    java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-owner.cache").toPath()));
                code.setPackageName(name);
                var setting = domainSetting((com.android.server.pm.pkg.AndroidPackage)code, false, id);
                com.android.server.pm.DomainApprovalFixture.ownersUser(setting, level, time);
                service.addPackage((com.android.server.pm.pkg.PackageStateInternal)setting, null);
                if (level != 6) settings.put(name, (com.android.server.pm.pkg.PackageStateInternal)setting);
            }
            if (in.dataAvail() != 0) throw new AssertionError("trailing grouped Owners users");
            var connection = new DomainConnection(settings); service.setConnection(connection);
            for (boolean v2 : new boolean[] {false, true}) {
                compat.v2 = v2;
                for (int user : new int[] {0, 10}) for (String host : new String[] {"h0.example", "unknown.invalid"}) {
                    var owners = service.getOwnersForDomain(host, user);
                    out.writeNoException(); out.writeInt(owners.size());
                    for (var owner : owners) out.writeTypedObject(owner, 0);
                }
            }
            java.nio.file.Files.write(new java.io.File(directory, "domain-owners-group.original").toPath(), out.marshall());
        } finally { compat.v2 = true; in.recycle(); out.recycle(); }
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
            var reply = android.os.Parcel.obtain();
            try {
                reply.writeNoException(); reply.writeTypedObject(info, 0);
                java.nio.file.Files.write(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".info").toPath(), reply.marshall());
            } finally { reply.recycle(); }
            out.writeInt(info == null ? 0 : 1);
            if (info != null) { out.writeString(info.getIdentifier().toString()); writeStates(out, info.getHostToStateMap()); }
            for (int user : new int[] {0, 10}) {
                connection.user = user;
                var selection = service.getDomainVerificationUserState("fixture.domains", user);
                if (selection == null) throw new AssertionError("missing attached user state");
                var userReply = android.os.Parcel.obtain();
                try {
                    userReply.writeNoException(); userReply.writeTypedObject(selection, 0);
                    java.nio.file.Files.write(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".user-" + user).toPath(), userReply.marshall());
                } finally { userReply.recycle(); }
                out.writeInt(selection.isLinkHandlingAllowed() ? 1 : 0);
                writeStates(out, selection.getHostToStateMap());
            }
            var names = service.queryValidVerificationPackageNames();
            out.writeInt(names.size());
            for (String name : names) out.writeString(name);
            if (connection.writes != 0) throw new AssertionError("query scheduled a settings write");
            java.nio.file.Files.write(new java.io.File(directory, "domain-owner-" + caseId + "-" + stage + ".queries").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    private DomainCollectorOracle() {}
}
