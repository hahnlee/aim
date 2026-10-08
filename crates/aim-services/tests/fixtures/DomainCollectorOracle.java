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
        DomainUuidOracle.verify(directory);
        UriDtoOracle.verify(directory);
        verifyAttachment(directory, context, compat);
        verifySettingsReadMerge(directory, context, compat);
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
    private static void verifySettingsReadMerge(java.io.File directory, android.content.Context context, Compat compat) throws Exception {
        compat.restricted = true;
        var code = (com.android.server.pm.pkg.AndroidPackage) com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-owner.cache").toPath()));
        for (int i = 0; i < 32; i++) {
            var service = new com.android.server.pm.verify.domain.DomainVerificationService(context, new com.android.server.SystemConfig(false), compat);
            var setting = domainSetting(code, false, "00000000-0000-0000-0000-000000000001"); service.addPackage((com.android.server.pm.pkg.PackageStateInternal)setting, null);
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-read-merge-seed"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readSettings(new DomainComputer(setting), parser);
            }
            boolean present = new String(java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-read-merge-" + i + ".code").toPath()), java.nio.charset.StandardCharsets.UTF_8).equals("true");
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-read-merge-" + i + ".input"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); service.readSettings(new DomainComputer(present ? java.util.Map.of("fixture.domains", (com.android.server.pm.pkg.PackageStateInternal)setting) : java.util.Map.of()), parser);
            }
            try (var output = new java.io.FileOutputStream(new java.io.File(directory, "domain-read-merge-" + i + ".original"))) {
                var xml = android.util.Xml.resolveSerializer(output); xml.startDocument(null,true); xml.startTag(null,"packages"); service.writeSettings(null,xml,false,-1); xml.endTag(null,"packages"); xml.endDocument();
            }
        }
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
            var binderMismatch = new android.os.Bundle();
            binderMismatch.putBinder("h0.example", new android.os.Binder());
            service.setUriRelativeFilterGroups("fixture.domains", binderMismatch);
            if (!service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("h0.example")).keySet().isEmpty()) throw new AssertionError("original Binder mismatch did not remove URI key");
            if (connection.writes != 0) throw new AssertionError("Binder URI mismatch scheduled persistence");
            for (int mode = 6; mode < 10; mode++) {
                service.setUriRelativeFilterGroups("fixture.domains", UriDtoOracle.styledBundle("h0.example", mode));
                if (!service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("h0.example")).keySet().isEmpty() || connection.writes != 0) throw new AssertionError("styled URI mismatch/removal or persistence differs");
            }
            var restoredGroup = new android.os.Bundle();
            restoredGroup.putParcelableArrayList("h0.example", new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(0, "/second")))));
            service.setUriRelativeFilterGroups("fixture.domains", restoredGroup);
            var nullableUpdate = new android.os.Bundle();
            var nullableGroup = new android.content.UriRelativeFilterGroupParcel(); nullableGroup.action = 0; nullableGroup.filters = new java.util.ArrayList<>();
            for (String value : new String[] {null, "", "ordinary"}) {
                var filter = new android.content.UriRelativeFilterParcel(); filter.filter = value; nullableGroup.filters.add(filter);
            }
            var nullableGroups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); nullableGroups.add(nullableGroup);
            nullableUpdate.putParcelableArrayList("h0.example", nullableGroups);
            service.setUriRelativeFilterGroups("fixture.domains", nullableUpdate);
            writeDomains(directory, i, "uri-null-write", service);
            var liveGroups = service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("h0.example")).getParcelableArrayList("h0.example", android.content.UriRelativeFilterGroupParcel.class);
            if (liveGroups.get(0).filters.size() != 3 || liveGroups.get(0).filters.get(0).filter != null) throw new AssertionError("XML write changed nullable runtime filters");
            var restoredService = new com.android.server.pm.verify.domain.DomainVerificationService(context, config, compat);
            try (var stream = new java.io.FileInputStream(new java.io.File(directory, "domain-owner-" + i + "-uri-null-write.original"))) {
                var parser = android.util.Xml.resolvePullParser(stream); parser.next(); restoredService.readSettings(null, parser);
            }
            writeDomains(directory, i, "uri-null-restore", restoredService);
            service.setUriRelativeFilterGroups("fixture.domains", restoredGroup);
            for (boolean followingGroup : new boolean[] {false, true}) {
                try {service.setUriRelativeFilterGroups("fixture.domains", UriDtoOracle.nullNamedBundle("h0.example", followingGroup)); throw new AssertionError("null Parcelable name accepted as URI group");}
                catch (NullPointerException expected) {}
                if (service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("h0.example")).keySet().isEmpty() || connection.writes != 0) throw new AssertionError("null Parcelable name changed URI key or persistence");
            }



            var keyErrors = android.os.Parcel.obtain();
            try {
                keyErrors.writeInt(2);
                for (String key : new String[] {null, ""}) {
                    var keyUpdate = new android.os.Bundle();
                    var changed = new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(1, "/key"))));
                    keyUpdate.putParcelableArrayList("h0.example", changed); keyUpdate.putString(key, "wrong type"); keyUpdate.putParcelableArrayList("a.example", changed);
                    try {service.setUriRelativeFilterGroups("fixture.domains", keyUpdate); throw new AssertionError("invalid URI key accepted");}
                    catch (NullPointerException | IndexOutOfBoundsException error) {
                        keyErrors.writeString(key); keyErrors.writeString(error.getClass().getName()); keyErrors.writeString(error.getMessage());
                    }
                    var afterKey = service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("a.example", "h0.example"));
                    if (afterKey.getParcelableArrayList("a.example", android.content.UriRelativeFilterGroupParcel.class).get(0).action != 1 || afterKey.getParcelableArrayList("h0.example", android.content.UriRelativeFilterGroupParcel.class).get(0).action != 0) throw new AssertionError("invalid key partial mutation differs");
                    var clearKey = new android.os.Bundle(); clearKey.putParcelableArrayList("a.example", null); service.setUriRelativeFilterGroups("fixture.domains", clearKey);
                    if (connection.writes != 0) throw new AssertionError("invalid URI key scheduled persistence");
                }
                java.nio.file.Files.write(new java.io.File(directory, "domain-uri-key-errors.original").toPath(), keyErrors.marshall());
            } finally {keyErrors.recycle();}
            for (String name : new String[] {"dev.aim.fixture.NoSuch", "java.lang.String", "android.content.UriRelativeFilterParcel", "android.os.Bundle", "[I"}) for (boolean root : new boolean[] {false, true}) {
                var classSeed = new android.os.Bundle(); classSeed.putParcelableArrayList("class.example", new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(1, "/class")))));
                service.setUriRelativeFilterGroups("fixture.domains", classSeed);
                boolean mismatch = name.equals("android.content.UriRelativeFilterParcel") || name.equals("android.os.Bundle");
                try {service.setUriRelativeFilterGroups("fixture.domains", UriDtoOracle.namedBundle("class.example", name, root)); if (!mismatch) throw new AssertionError("bad Parcelable class accepted");}
                catch (RuntimeException error) {if (mismatch || !"android.os.BadParcelableException".equals(error.getClass().getName())) throw error;}
                if (service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("class.example")).keySet().contains("class.example") == mismatch || connection.writes != 0) throw new AssertionError("Parcelable mismatch/removal/error persistence differs");
            }
            var classCleanup = new android.os.Bundle(); classCleanup.putParcelableArrayList("class.example", null); service.setUriRelativeFilterGroups("fixture.domains", classCleanup);
            for (String name : new String[] {null, "dev.aim.fixture.NoSuch", "java.lang.String"}) for (boolean root : new boolean[] {false, true}) {
                var serialSeed = new android.os.Bundle(); serialSeed.putParcelableArrayList("serial.example", new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(1, "/serial")))));
                service.setUriRelativeFilterGroups("fixture.domains", serialSeed);
                boolean errorExpected = name != null || !root;
                try {service.setUriRelativeFilterGroups("fixture.domains", UriDtoOracle.serialBundle("serial.example", name, root)); if (errorExpected) throw new AssertionError("malformed Serializable accepted");}
                catch (RuntimeException error) {
                    String expectedClass = name == null ? "java.lang.NullPointerException" : "android.os.BadParcelableException";
                    if (!errorExpected || !expectedClass.equals(error.getClass().getName())) throw error;
                }
                if (service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("serial.example")).keySet().contains("serial.example") != errorExpected || connection.writes != 0) throw new AssertionError("Serializable error/null state differs");
            }
            var serialCleanup = new android.os.Bundle(); serialCleanup.putParcelableArrayList("serial.example", null); service.setUriRelativeFilterGroups("fixture.domains", serialCleanup);
            Object[] validValues = {"wrong", new java.util.ArrayList<>(), new java.util.ArrayList<>(java.util.Arrays.asList((Object) null)), new java.util.ArrayList<>(java.util.List.of("wrong"))};
            for (int mode = 0; mode < validValues.length; mode++) for (boolean root : new boolean[] {false, true}) {
                var validSeed = new android.os.Bundle(); validSeed.putParcelableArrayList("validserial.example", new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(1, "/validserial"))))); service.setUriRelativeFilterGroups("fixture.domains", validSeed);
                var bytes = new java.io.ByteArrayOutputStream(); try (var stream = new java.io.ObjectOutputStream(bytes)) {stream.writeObject(validValues[mode]);}
                boolean errorExpected = root && mode >= 2;
                try {service.setUriRelativeFilterGroups("fixture.domains", UriDtoOracle.serialPayloadBundle("validserial.example", validValues[mode].getClass().getName(), bytes.toByteArray(), root)); if (errorExpected) throw new AssertionError("serialized invalid element accepted");}
                catch (RuntimeException error) {if (!errorExpected || (mode == 2 ? !(error instanceof NullPointerException) : !(error instanceof ClassCastException))) throw error;}
                if (service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("validserial.example")).keySet().contains("validserial.example") != errorExpected || connection.writes != 0) throw new AssertionError("valid Serializable state differs");
            }
            var validCleanup = new android.os.Bundle(); validCleanup.putParcelableArrayList("validserial.example", null); service.setUriRelativeFilterGroups("fixture.domains", validCleanup);
            var seed = new android.os.Bundle();
            var oldGroups = new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(0, "/old"))));
            seed.putParcelableArrayList("runtime.example", oldGroups); seed.putParcelableArrayList("late.example", oldGroups);
            service.setUriRelativeFilterGroups("fixture.domains", seed);
            var partial = new android.os.Bundle();
            var newGroups = new java.util.ArrayList<>(android.content.UriRelativeFilterGroup.groupsToParcels(java.util.List.of(uriGroup(1, "/new"))));
            partial.putParcelableArrayList("late.example", newGroups);
            var badGroups = new java.util.ArrayList<android.content.UriRelativeFilterGroupParcel>(); badGroups.add(null);
            partial.putParcelableArrayList("runtime.example", badGroups); partial.putParcelableArrayList("partial.example", newGroups);
            try {service.setUriRelativeFilterGroups("fixture.domains", partial); throw new AssertionError("partial URI conversion accepted");}
            catch (NullPointerException expected) {}
            var after = service.getUriRelativeFilterGroups("fixture.domains", java.util.List.of("partial.example", "runtime.example", "late.example"));
            for (String host : java.util.List.of("partial.example", "runtime.example", "late.example")) {
                var groups = after.getParcelableArrayList(host, android.content.UriRelativeFilterGroupParcel.class);
                if (groups == null || groups.size() != 1 || groups.get(0).action != (host.equals("partial.example") ? 1 : 0)) throw new AssertionError("partial URI mutation order differs");
            }
            if (connection.writes != 0) throw new AssertionError("partial URI failure scheduled persistence");
            var cleanup = new android.os.Bundle();
            for (String host : java.util.List.of("partial.example", "runtime.example", "late.example")) cleanup.putParcelableArrayList(host, null);
            service.setUriRelativeFilterGroups("fixture.domains", cleanup);

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
        public com.android.server.pm.pkg.PackageStateInternal getPackageStateInternal(String name, int callingUid) { throw new AssertionError("unexpected domain fixture Computer.getPackageStateInternal with UID"); }
        public int getVersion() { throw new AssertionError("unexpected domain fixture Computer.getVersion"); }
        public com.android.server.pm.Computer use() { throw new AssertionError("unexpected domain fixture Computer.use"); }
        public java.util.List<android.content.pm.ResolveInfo> queryIntentActivitiesInternal(android.content.Intent intent, String resolvedType, long flags, long privateResolveFlags, int filterCallingUid, int callingPid, int userId, boolean resolveForStart, boolean allowDynamicSplits) { throw new AssertionError("unexpected domain fixture Computer.queryIntentActivitiesInternal"); }
        public java.util.List<android.content.pm.ResolveInfo> queryIntentActivitiesInternal(android.content.Intent intent, String resolvedType, long flags, int filterCallingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.queryIntentActivitiesInternal"); }
        public java.util.List<android.content.pm.ResolveInfo> queryIntentActivitiesInternal(android.content.Intent intent, String resolvedType, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.queryIntentActivitiesInternal"); }
        public java.util.List<android.content.pm.ResolveInfo> queryIntentServicesInternal(android.content.Intent intent, String resolvedType, long flags, int userId, int callingUid, int callingPid, boolean includeInstantApps, boolean resolveForStart) { throw new AssertionError("unexpected domain fixture Computer.queryIntentServicesInternal"); }
        public android.content.pm.ActivityInfo getActivityInfo(android.content.ComponentName component, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getActivityInfo"); }
        public android.content.pm.ActivityInfo getActivityInfoInternal(android.content.ComponentName component, long flags, int filterCallingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getActivityInfoInternal"); }
        public com.android.server.pm.pkg.AndroidPackage getPackage(String packageName) { throw new AssertionError("unexpected domain fixture Computer.getPackage"); }
        public com.android.server.pm.pkg.AndroidPackage getPackage(int uid) { throw new AssertionError("unexpected domain fixture Computer.getPackage"); }
        public android.content.pm.ApplicationInfo getApplicationInfo(String packageName, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getApplicationInfo"); }
        public android.content.pm.ApplicationInfo getApplicationInfoInternal(String packageName, long flags, int filterCallingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getApplicationInfoInternal"); }
        public android.content.ComponentName getDefaultHomeActivity(int userId) { throw new AssertionError("unexpected domain fixture Computer.getDefaultHomeActivity"); }
        public android.content.ComponentName getHomeActivitiesAsUser(java.util.List<android.content.pm.ResolveInfo> allHomeCandidates, int userId) { throw new AssertionError("unexpected domain fixture Computer.getHomeActivitiesAsUser"); }
        public com.android.server.pm.CrossProfileDomainInfo getCrossProfileDomainPreferredLpr(android.content.Intent intent, String resolvedType, long flags, int sourceUserId, int parentUserId) { throw new AssertionError("unexpected domain fixture Computer.getCrossProfileDomainPreferredLpr"); }
        public android.content.Intent getHomeIntent() { throw new AssertionError("unexpected domain fixture Computer.getHomeIntent"); }
        public java.util.List<com.android.server.pm.CrossProfileIntentFilter> getMatchingCrossProfileIntentFilters(android.content.Intent intent, String resolvedType, int userId) { throw new AssertionError("unexpected domain fixture Computer.getMatchingCrossProfileIntentFilters"); }
        public java.util.List<android.content.pm.ResolveInfo> applyPostResolutionFilter(java.util.List<android.content.pm.ResolveInfo> resolveInfos, String ephemeralPkgName, boolean allowDynamicSplits, int filterCallingUid, boolean resolveForStart, int userId, android.content.Intent intent) { throw new AssertionError("unexpected domain fixture Computer.applyPostResolutionFilter"); }
        public android.content.pm.PackageInfo getPackageInfo(String packageName, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageInfo"); }
        public android.content.pm.PackageInfo getPackageInfoInternal(String packageName, long versionCode, long flags, int filterCallingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageInfoInternal"); }
        public String[] getAllAvailablePackageNames() { throw new AssertionError("unexpected domain fixture Computer.getAllAvailablePackageNames"); }
        public com.android.server.pm.pkg.PackageStateInternal getPackageStateFiltered(String packageName, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageStateFiltered"); }
        public android.content.pm.ParceledListSlice<android.content.pm.PackageInfo> getInstalledPackages(long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getInstalledPackages"); }
        public android.content.pm.ResolveInfo createForwardingResolveInfoUnchecked(com.android.server.pm.WatchedIntentFilter filter, int sourceUserId, int targetUserId) { throw new AssertionError("unexpected domain fixture Computer.createForwardingResolveInfoUnchecked"); }
        public android.content.pm.ServiceInfo getServiceInfo(android.content.ComponentName component, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getServiceInfo"); }
        public android.content.pm.SharedLibraryInfo getSharedLibraryInfo(String name, long version) { throw new AssertionError("unexpected domain fixture Computer.getSharedLibraryInfo"); }
        public String getInstantAppPackageName(int callingUid) { throw new AssertionError("unexpected domain fixture Computer.getInstantAppPackageName"); }
        public String resolveInternalPackageName(String packageName, long versionCode) { throw new AssertionError("unexpected domain fixture Computer.resolveInternalPackageName"); }
        public String[] getPackagesForUid(int uid) { throw new AssertionError("unexpected domain fixture Computer.getPackagesForUid"); }
        public android.content.pm.UserInfo getProfileParent(int userId) { throw new AssertionError("unexpected domain fixture Computer.getProfileParent"); }
        public boolean canViewInstantApps(int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.canViewInstantApps"); }
        public boolean isCallerSameApp(String packageName, int uid) { throw new AssertionError("unexpected domain fixture Computer.isCallerSameApp"); }
        public boolean isCallerSameApp(String packageName, int uid, boolean resolveIsolatedUid) { throw new AssertionError("unexpected domain fixture Computer.isCallerSameApp"); }
        public boolean isImplicitImageCaptureIntentAndNotSetByDpc(android.content.Intent intent, int userId, String resolvedType, long flags) { throw new AssertionError("unexpected domain fixture Computer.isImplicitImageCaptureIntentAndNotSetByDpc"); }
        public boolean isInstantApp(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.isInstantApp"); }
        public boolean isInstantAppInternal(String packageName, int userId, int callingUid) { throw new AssertionError("unexpected domain fixture Computer.isInstantAppInternal"); }
        public boolean shouldFilterApplication(com.android.server.pm.pkg.PackageStateInternal ps, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.shouldFilterApplication"); }
        public boolean shouldFilterApplicationIncludingUninstalled(com.android.server.pm.pkg.PackageStateInternal ps, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.shouldFilterApplicationIncludingUninstalled"); }
        public int checkUidPermission(String permName, int uid) { throw new AssertionError("unexpected domain fixture Computer.checkUidPermission"); }
        public int getPackageUidInternal(String packageName, long flags, int userId, int callingUid) { throw new AssertionError("unexpected domain fixture Computer.getPackageUidInternal"); }
        public long updateFlagsForResolve(long flags, int userId, int callingUid, boolean wantInstantApps, boolean isImplicitImageCaptureIntentAndNotSetByDpc) { throw new AssertionError("unexpected domain fixture Computer.updateFlagsForResolve"); }
        public void enforceCrossUserOrProfilePermission(int callingUid, int userId, boolean requireFullPermission, boolean checkShell, String message) { throw new AssertionError("unexpected domain fixture Computer.enforceCrossUserOrProfilePermission"); }
        public void enforceCrossUserPermission(int callingUid, int userId, boolean requireFullPermission, boolean checkShell, String message) { throw new AssertionError("unexpected domain fixture Computer.enforceCrossUserPermission"); }
        public android.content.pm.SigningDetails getSigningDetails(String packageName) { throw new AssertionError("unexpected domain fixture Computer.getSigningDetails"); }
        public android.content.pm.SigningDetails getSigningDetails(int uid) { throw new AssertionError("unexpected domain fixture Computer.getSigningDetails"); }
        public boolean filterAppAccess(com.android.server.pm.pkg.AndroidPackage pkg, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.filterAppAccess"); }
        public boolean filterAppAccess(String packageName, int callingUid, int userId, boolean filterUninstalled) { throw new AssertionError("unexpected domain fixture Computer.filterAppAccess"); }
        public boolean filterAppAccess(int uid, int callingUid) { throw new AssertionError("unexpected domain fixture Computer.filterAppAccess"); }
        public void dump(int type, java.io.FileDescriptor fd, java.io.PrintWriter pw, com.android.server.pm.DumpState dumpState) { throw new AssertionError("unexpected domain fixture Computer.dump"); }
        public com.android.server.pm.PackageManagerService.FindPreferredActivityBodyResult findPreferredActivityInternal(android.content.Intent intent, String resolvedType, long flags, java.util.List<android.content.pm.ResolveInfo> query, boolean always, boolean removeMatches, boolean debug, int userId, boolean queryMayBeFiltered) { throw new AssertionError("unexpected domain fixture Computer.findPreferredActivityInternal"); }
        public android.content.pm.ResolveInfo findPersistentPreferredActivity(android.content.Intent intent, String resolvedType, long flags, java.util.List<android.content.pm.ResolveInfo> query, boolean debug, int userId) { throw new AssertionError("unexpected domain fixture Computer.findPersistentPreferredActivity"); }
        public com.android.server.pm.PreferredIntentResolver getPreferredActivities(int userId) { throw new AssertionError("unexpected domain fixture Computer.getPreferredActivities"); }
        public android.util.ArrayMap<String, ? extends com.android.server.pm.pkg.PackageStateInternal> getPackageStates() { throw new AssertionError("unexpected domain fixture Computer.getPackageStates"); }
        public android.util.ArrayMap<String, ? extends com.android.server.pm.pkg.PackageStateInternal> getDisabledSystemPackageStates() { throw new AssertionError("unexpected domain fixture Computer.getDisabledSystemPackageStates"); }
        public android.util.ArraySet<String> getNotifyPackagesForReplacedReceived(String[] packages) { throw new AssertionError("unexpected domain fixture Computer.getNotifyPackagesForReplacedReceived"); }
        public int getPackageStartability(boolean safeMode, String packageName, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageStartability"); }
        public boolean isPackageAvailable(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.isPackageAvailable"); }
        public boolean isApexPackage(String packageName) { throw new AssertionError("unexpected domain fixture Computer.isApexPackage"); }
        public String[] currentToCanonicalPackageNames(String[] names) { throw new AssertionError("unexpected domain fixture Computer.currentToCanonicalPackageNames"); }
        public String[] canonicalToCurrentPackageNames(String[] names) { throw new AssertionError("unexpected domain fixture Computer.canonicalToCurrentPackageNames"); }
        public int[] getPackageGids(String packageName, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageGids"); }
        public int getTargetSdkVersion(String packageName) { throw new AssertionError("unexpected domain fixture Computer.getTargetSdkVersion"); }
        public boolean activitySupportsIntentAsUser(android.content.ComponentName resolveComponentName, android.content.ComponentName component, android.content.Intent intent, String resolvedType, int userId) { throw new AssertionError("unexpected domain fixture Computer.activitySupportsIntentAsUser"); }
        public android.content.pm.ActivityInfo getReceiverInfo(android.content.ComponentName component, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getReceiverInfo"); }
        public android.content.pm.ParceledListSlice<android.content.pm.SharedLibraryInfo> getSharedLibraries(String packageName, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getSharedLibraries"); }
        public boolean canRequestPackageInstalls(String packageName, int callingUid, int userId, boolean throwIfPermNotDeclared) { throw new AssertionError("unexpected domain fixture Computer.canRequestPackageInstalls"); }
        public boolean isInstallDisabledForPackage(String packageName, int uid, int userId) { throw new AssertionError("unexpected domain fixture Computer.isInstallDisabledForPackage"); }
        public android.util.Pair<java.util.List<android.content.pm.VersionedPackage>, java.util.List<Boolean>> getPackagesUsingSharedLibrary(android.content.pm.SharedLibraryInfo libInfo, long flags, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackagesUsingSharedLibrary"); }
        public android.content.pm.ParceledListSlice<android.content.pm.SharedLibraryInfo> getDeclaredSharedLibraries(String packageName, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getDeclaredSharedLibraries"); }
        public android.content.pm.ProviderInfo getProviderInfo(android.content.ComponentName component, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getProviderInfo"); }
        public android.util.ArrayMap<String, String> getSystemSharedLibraryNamesAndPaths() { throw new AssertionError("unexpected domain fixture Computer.getSystemSharedLibraryNamesAndPaths"); }
        public com.android.server.pm.pkg.PackageStateInternal getPackageStateForInstalledAndFiltered(String packageName, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageStateForInstalledAndFiltered"); }
        public int checkSignatures(String pkg1, String pkg2, int userId) { throw new AssertionError("unexpected domain fixture Computer.checkSignatures"); }
        public int checkUidSignatures(int uid1, int uid2) { throw new AssertionError("unexpected domain fixture Computer.checkUidSignatures"); }
        public int checkUidSignaturesForAllUsers(int uid1, int uid2) { throw new AssertionError("unexpected domain fixture Computer.checkUidSignaturesForAllUsers"); }
        public boolean hasSigningCertificate(String packageName, byte[] certificate, int type) { throw new AssertionError("unexpected domain fixture Computer.hasSigningCertificate"); }
        public boolean hasUidSigningCertificate(int uid, byte[] certificate, int type) { throw new AssertionError("unexpected domain fixture Computer.hasUidSigningCertificate"); }
        public java.util.List<String> getAllPackages() { throw new AssertionError("unexpected domain fixture Computer.getAllPackages"); }
        public String getNameForUid(int uid) { throw new AssertionError("unexpected domain fixture Computer.getNameForUid"); }
        public String[] getNamesForUids(int[] uids) { throw new AssertionError("unexpected domain fixture Computer.getNamesForUids"); }
        public int getUidForSharedUser(String sharedUserName) { throw new AssertionError("unexpected domain fixture Computer.getUidForSharedUser"); }
        public int getFlagsForUid(int uid) { throw new AssertionError("unexpected domain fixture Computer.getFlagsForUid"); }
        public int getPrivateFlagsForUid(int uid) { throw new AssertionError("unexpected domain fixture Computer.getPrivateFlagsForUid"); }
        public boolean isUidPrivileged(int uid) { throw new AssertionError("unexpected domain fixture Computer.isUidPrivileged"); }
        public String[] getAppOpPermissionPackages(String permissionName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getAppOpPermissionPackages"); }
        public android.content.pm.ParceledListSlice<android.content.pm.PackageInfo> getPackagesHoldingPermissions(String[] permissions, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackagesHoldingPermissions"); }
        public java.util.List<android.content.pm.ApplicationInfo> getInstalledApplications(long flags, int userId, int callingUid, boolean forceAllowCrossUser) { throw new AssertionError("unexpected domain fixture Computer.getInstalledApplications"); }
        public android.content.pm.ProviderInfo resolveContentProvider(String name, long flags, int userId, int callingUid) { throw new AssertionError("unexpected domain fixture Computer.resolveContentProvider"); }
        public android.content.pm.ProviderInfo resolveContentProviderForUid(String name, long flags, int userId, int filterCallingUid) { throw new AssertionError("unexpected domain fixture Computer.resolveContentProviderForUid"); }
        public android.content.pm.ProviderInfo getGrantImplicitAccessProviderInfo(int recipientUid, String visibleAuthority) { throw new AssertionError("unexpected domain fixture Computer.getGrantImplicitAccessProviderInfo"); }
        public void querySyncProviders(boolean safeMode, java.util.List<String> outNames, java.util.List<android.content.pm.ProviderInfo> outInfo) { throw new AssertionError("unexpected domain fixture Computer.querySyncProviders"); }
        public android.content.pm.ParceledListSlice<android.content.pm.ProviderInfo> queryContentProviders(String processName, int uid, long flags, String metaDataKey) { throw new AssertionError("unexpected domain fixture Computer.queryContentProviders"); }
        public android.content.pm.InstrumentationInfo getInstrumentationInfoAsUser(android.content.ComponentName component, int flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getInstrumentationInfoAsUser"); }
        public android.content.pm.ParceledListSlice<android.content.pm.InstrumentationInfo> queryInstrumentationAsUser(String targetPackage, int flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.queryInstrumentationAsUser"); }
        public java.util.List<com.android.server.pm.pkg.PackageStateInternal> findSharedNonSystemLibraries(com.android.server.pm.pkg.PackageStateInternal pkgSetting) { throw new AssertionError("unexpected domain fixture Computer.findSharedNonSystemLibraries"); }
        public boolean getApplicationHiddenSettingAsUser(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getApplicationHiddenSettingAsUser"); }
        public boolean isPackageSuspendedForUser(String packageName, int userId) throws android.content.pm.PackageManager.NameNotFoundException { throw new AssertionError("unexpected domain fixture Computer.isPackageSuspendedForUser"); }
        public boolean isPackageQuarantinedForUser(String packageName, int userId) throws android.content.pm.PackageManager.NameNotFoundException { throw new AssertionError("unexpected domain fixture Computer.isPackageQuarantinedForUser"); }
        public boolean isPackageStoppedForUser(String packageName, int userId) throws android.content.pm.PackageManager.NameNotFoundException { throw new AssertionError("unexpected domain fixture Computer.isPackageStoppedForUser"); }
        public boolean isSuspendingAnyPackages(String suspendingPackage, int suspendingUserId, int targetUserId) { throw new AssertionError("unexpected domain fixture Computer.isSuspendingAnyPackages"); }
        public android.content.pm.ParceledListSlice<android.content.IntentFilter> getAllIntentFilters(String packageName) { throw new AssertionError("unexpected domain fixture Computer.getAllIntentFilters"); }
        public boolean getBlockUninstallForUser(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getBlockUninstallForUser"); }
        public String getInstallerPackageName(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getInstallerPackageName"); }
        public android.content.pm.InstallSourceInfo getInstallSourceInfo(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getInstallSourceInfo"); }
        public int getApplicationEnabledSetting(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getApplicationEnabledSetting"); }
        public int getComponentEnabledSetting(android.content.ComponentName component, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getComponentEnabledSetting"); }
        public int getComponentEnabledSettingInternal(android.content.ComponentName component, int callingUid, int userId) { throw new AssertionError("unexpected domain fixture Computer.getComponentEnabledSettingInternal"); }
        public boolean isComponentEffectivelyEnabled(android.content.pm.ComponentInfo componentInfo, android.os.UserHandle userHandle) { throw new AssertionError("unexpected domain fixture Computer.isComponentEffectivelyEnabled"); }
        public boolean isApplicationEffectivelyEnabled(String packageName, android.os.UserHandle userHandle) { throw new AssertionError("unexpected domain fixture Computer.isApplicationEffectivelyEnabled"); }
        public android.content.pm.KeySet getKeySetByAlias(String packageName, String alias) { throw new AssertionError("unexpected domain fixture Computer.getKeySetByAlias"); }
        public android.content.pm.KeySet getSigningKeySet(String packageName) { throw new AssertionError("unexpected domain fixture Computer.getSigningKeySet"); }
        public boolean isPackageSignedByKeySet(String packageName, android.content.pm.KeySet ks) { throw new AssertionError("unexpected domain fixture Computer.isPackageSignedByKeySet"); }
        public boolean isPackageSignedByKeySetExactly(String packageName, android.content.pm.KeySet ks) { throw new AssertionError("unexpected domain fixture Computer.isPackageSignedByKeySetExactly"); }
        public android.util.SparseArray<int[]> getVisibilityAllowLists(String packageName, int[] userIds) { throw new AssertionError("unexpected domain fixture Computer.getVisibilityAllowLists"); }
        public int[] getVisibilityAllowList(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getVisibilityAllowList"); }
        public boolean canQueryPackage(int callingUid, String targetPackageName) { throw new AssertionError("unexpected domain fixture Computer.canQueryPackage"); }
        public int getPackageUid(String packageName, long flags, int userId) { throw new AssertionError("unexpected domain fixture Computer.getPackageUid"); }
        public boolean canAccessComponent(int callingUid, android.content.ComponentName component, int userId) { throw new AssertionError("unexpected domain fixture Computer.canAccessComponent"); }
        public boolean isCallerInstallerOfRecord(com.android.server.pm.pkg.AndroidPackage pkg, int callingUid) { throw new AssertionError("unexpected domain fixture Computer.isCallerInstallerOfRecord"); }
        public int getInstallReason(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getInstallReason"); }
        public boolean[] canPackageQuery(String sourcePackageName, String[] targetPackageNames, int userId) { throw new AssertionError("unexpected domain fixture Computer.canPackageQuery"); }
        public boolean canForwardTo(android.content.Intent intent, String resolvedType, int sourceUserId, int targetUserId) { throw new AssertionError("unexpected domain fixture Computer.canForwardTo"); }
        public java.util.List<android.content.pm.ApplicationInfo> getPersistentApplications(boolean safeMode, int flags) { throw new AssertionError("unexpected domain fixture Computer.getPersistentApplications"); }
        public String[] getSharedUserPackagesForPackage(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getSharedUserPackagesForPackage"); }
        public CharSequence getHarmfulAppWarning(String packageName, int userId) { throw new AssertionError("unexpected domain fixture Computer.getHarmfulAppWarning"); }
        public String[] filterOnlySystemPackages(String[] pkgNames) { throw new AssertionError("unexpected domain fixture Computer.filterOnlySystemPackages"); }
        public java.util.List<com.android.server.pm.pkg.AndroidPackage> getPackagesForAppId(int appId) { throw new AssertionError("unexpected domain fixture Computer.getPackagesForAppId"); }
        public int getUidTargetSdkVersion(int uid) { throw new AssertionError("unexpected domain fixture Computer.getUidTargetSdkVersion"); }
        public android.util.ArrayMap<String, android.content.pm.ProcessInfo> getProcessesForUid(int uid) { throw new AssertionError("unexpected domain fixture Computer.getProcessesForUid"); }
        public boolean getBlockUninstall(int userId, String packageName) { throw new AssertionError("unexpected domain fixture Computer.getBlockUninstall"); }
        public com.android.server.utils.WatchedArrayMap<String, com.android.server.utils.WatchedLongSparseArray<android.content.pm.SharedLibraryInfo>> getSharedLibraries() { throw new AssertionError("unexpected domain fixture Computer.getSharedLibraries"); }
        public android.util.Pair<com.android.server.pm.pkg.PackageStateInternal, com.android.server.pm.pkg.SharedUserApi> getPackageOrSharedUser(int appId) { throw new AssertionError("unexpected domain fixture Computer.getPackageOrSharedUser"); }
        public com.android.server.pm.pkg.SharedUserApi getSharedUser(int sharedUserAppIde) { throw new AssertionError("unexpected domain fixture Computer.getSharedUser"); }
        public android.util.ArraySet<com.android.server.pm.pkg.PackageStateInternal> getSharedUserPackages(int sharedUserAppId) { throw new AssertionError("unexpected domain fixture Computer.getSharedUserPackages"); }
        public com.android.server.pm.resolution.ComponentResolverApi getComponentResolver() { throw new AssertionError("unexpected domain fixture Computer.getComponentResolver"); }
        public com.android.server.pm.pkg.PackageStateInternal getDisabledSystemPackage(String packageName) { throw new AssertionError("unexpected domain fixture Computer.getDisabledSystemPackage"); }
        public android.content.pm.ResolveInfo getInstantAppInstallerInfo() { throw new AssertionError("unexpected domain fixture Computer.getInstantAppInstallerInfo"); }
        public com.android.server.utils.WatchedArrayMap<String, Integer> getFrozenPackages() { throw new AssertionError("unexpected domain fixture Computer.getFrozenPackages"); }
        public void checkPackageFrozen(String packageName) { throw new AssertionError("unexpected domain fixture Computer.checkPackageFrozen"); }
        public android.content.ComponentName getInstantAppInstallerComponent() { throw new AssertionError("unexpected domain fixture Computer.getInstantAppInstallerComponent"); }
        public void dumpPermissions(java.io.PrintWriter pw, String packageName, android.util.ArraySet<String> permissionNames, com.android.server.pm.DumpState dumpState) { throw new AssertionError("unexpected domain fixture Computer.dumpPermissions"); }
        public void dumpPackages(java.io.PrintWriter pw, String packageName, android.util.ArraySet<String> permissionNames, com.android.server.pm.DumpState dumpState, boolean checkin) { throw new AssertionError("unexpected domain fixture Computer.dumpPackages"); }
        public void dumpKeySet(java.io.PrintWriter pw, String packageName, com.android.server.pm.DumpState dumpState) { throw new AssertionError("unexpected domain fixture Computer.dumpKeySet"); }
        public void dumpSharedUsers(java.io.PrintWriter pw, String packageName, android.util.ArraySet<String> permissionNames, com.android.server.pm.DumpState dumpState, boolean checkin) { throw new AssertionError("unexpected domain fixture Computer.dumpSharedUsers"); }
        public void dumpSharedUsersProto(android.util.proto.ProtoOutputStream proto) { throw new AssertionError("unexpected domain fixture Computer.dumpSharedUsersProto"); }
        public void dumpPackagesProto(android.util.proto.ProtoOutputStream proto) { throw new AssertionError("unexpected domain fixture Computer.dumpPackagesProto"); }
        public void dumpSharedLibrariesProto(android.util.proto.ProtoOutputStream protoOutputStream) { throw new AssertionError("unexpected domain fixture Computer.dumpSharedLibrariesProto"); }
        public java.util.List<? extends com.android.server.pm.pkg.PackageStateInternal> getVolumePackages(String volumeUuid) { throw new AssertionError("unexpected domain fixture Computer.getVolumePackages"); }
        public android.content.pm.UserInfo[] getUserInfos() { throw new AssertionError("unexpected domain fixture Computer.getUserInfos"); }
        public android.util.ArrayMap<String, ? extends com.android.server.pm.pkg.SharedUserApi> getSharedUsers() { throw new AssertionError("unexpected domain fixture Computer.getSharedUsers"); }
    }
    public static void connectLegacySettingsReader(com.android.server.pm.verify.domain.DomainVerificationService service, com.android.server.pm.PackageSetting setting) {
        service.setConnection(new DomainConnection(java.util.Map.of(setting.getPackageName(), (com.android.server.pm.pkg.PackageStateInternal)setting)));
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
            if (!computer.settings.containsKey(name) || uid != 1000 || !doesUserExist(id)) throw new AssertionError("foreign domain visibility identity");
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
            var statuses = android.os.Parcel.obtain();
            try {
                int index = 0;
                for (int phase = 0; phase < 3; phase++) {
                    compat.v2 = phase != 2;
                    if (phase == 1) for (String suffix : new String[] {"Aa", "BB", "a", "A", "İ", "ı", "instant"})
                        service.setDomainVerificationLinkHandlingAllowedInternal("fixture.owner." + suffix, false, 0);
                    for (int user : new int[] {0, 10}) for (String suffix : new String[] {"disabled", "selected", "always", "Aa", "instant"})
                        for (boolean enabled : new boolean[] {true, false}) for (boolean multiple : new boolean[] {false, true}) {
                            String name = "fixture.owner." + suffix;
                            String id = new String(java.nio.file.Files.readAllBytes(new java.io.File(directory, "domain-selection-" + suffix + ".id").toPath()), java.nio.charset.StandardCharsets.UTF_8);
                            var hosts = new java.util.TreeSet<String>(); hosts.add("h0.example"); if (multiple) hosts.add("h1.example");
                            statuses.writeInt(service.setDomainVerificationUserSelection(java.util.UUID.fromString(id), hosts, enabled, user));
                            try (var output = new java.io.FileOutputStream(new java.io.File(directory, "domain-selection-" + index++ + ".original"))) {
                                var serializer = android.util.Xml.resolveSerializer(output); serializer.startDocument(null, true); serializer.startTag(null, "packages");
                                service.writeSettings(null, serializer, false, -1); serializer.endTag(null, "packages"); serializer.endDocument();
                            }
                        }
                }
                java.nio.file.Files.write(new java.io.File(directory, "domain-selection.status").toPath(), statuses.marshall());
            } finally { statuses.recycle(); }

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
