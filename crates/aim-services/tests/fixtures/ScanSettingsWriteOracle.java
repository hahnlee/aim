package com.android.server.pm;

/** Actual pinned Settings writer on detached, native-captured setting owners. */
public final class ScanSettingsWriteOracle {
    public static void write(java.io.File cache, dev.aim.server.PackageScanLease lease,
            PackageSetting assembled) throws Exception {
        verifyArrayMapOrder();
        verifyFirstWriteRetry(cache.getParentFile());
        verifyEmptyDocuments(cache.getParentFile());
        verifyRecoveryMatrix(cache.getParentFile());
        verifyPullMatrix(cache.getParentFile());
        verifySettingsVersionRecovery(cache.getParentFile());
        verifySettingsDefaults(cache.getParentFile());
        verifyOwnerDefaults(cache.getParentFile());
        verifySignatureEvents(cache.getParentFile());
        var in = android.os.Parcel.obtain();
        PackageSetting setting;
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(cache.getPath() + ".writer-setting").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var data = dev.aim.server.PackageSettingData.read(in);
            if (in.dataAvail() != 0) throw new AssertionError("writer setting tail");
            setting = CapturedPackageSetting.from(data, 1, false);
        } finally { in.recycle(); }
        setting.setPkg(((com.android.server.pm.pkg.PackageState)assembled).getAndroidPackage());
        setting.setSigningDetails(assembled.getSigningDetails());
        verifyReindexedCertificates(cache.getParentFile());
        verifyNativeSignatures(cache.getParentFile(), lease, setting);
        writeInventory(cache.getParentFile(), lease);
        var settings = new Settings(java.util.Map.of());
        var certificates = new java.util.ArrayList<android.content.pm.Signature>();
        try (var output = new java.io.FileOutputStream(cache.getPath() + ".settings-original")) {
            var xml = android.util.Xml.resolveSerializer(output);
            xml.startDocument(null, true); xml.startTag(null, "packages");
            settings.writePackageLPr(xml, certificates, setting);
            var names = new java.util.ArrayList<>(lease.getSharedUserNames());
            names.sort(java.util.Comparator.comparingInt(String::hashCode));
            for (String name : names) {
                var data = lease.getSharedUserData(name);
                var sigs = new PackageSignatures(); sigs.mSigningDetails = data.getSigningDetails();
                xml.startTag(null, "shared-user"); xml.attribute(null, "name", name);
                xml.attributeInt(null, "userId", data.getAppId());
                sigs.writeXml(xml, "sigs", certificates); xml.endTag(null, "shared-user");
            }
            xml.endTag(null, "packages"); xml.endDocument();
        }
    }
    private static void verifyOwnerDefaults(java.io.File directory) throws Exception {
        for (int index = 0; ; index++) {
            var input = new java.io.File(directory, "owner-default-input-" + index);
            if (!input.exists()) break;
            var data = new java.io.File(directory, "owner-default-original-" + index);
            var system = new java.io.File(data, "system"); system.mkdirs();
            var main = new java.io.File(system, "packages.xml");
            var reserve = new java.io.File(system, "packages.xml.reservecopy");
            java.nio.file.Files.write(main.toPath(), java.nio.file.Files.readAllBytes(input.toPath()));
            java.nio.file.Files.write(reserve.toPath(), "<packages/>".getBytes(java.nio.charset.StandardCharsets.UTF_8));
            var settings = new Settings(data, null, null, null, null, new PackageManagerTracedLock());
            var configured = new java.io.File(directory, "owner-default-configured-" + index).exists();
            com.android.server.pm.permission.LegacyPermission configuredPermission = null, configuredTree = null;
            if (configured) {
                configuredPermission = configuredPermission(); configuredTree = configuredPermission();
                settings.mPermissions.replacePermissions(java.util.List.of(configuredPermission));
                settings.mPermissions.replacePermissionTrees(java.util.List.of(configuredTree));
            }
            boolean first = !settings.readSettingsLPw(null, java.util.List.of(), new android.util.ArrayMap<>());
            if (configured && (!settings.mPermissions.getPermissions().contains(configuredPermission)
                    || !settings.mPermissions.getPermissionTrees().contains(configuredTree)))
                throw new AssertionError("configured permission identity replaced: " + index);
            var output = new java.util.ArrayList<String>(); output.add(Boolean.toString(first));
            var signatures = new java.util.ArrayList<String>();
            for (var setting : settings.getPackagesLocked().values()) signatures.add(setting.getPackageName() + ":" + signatureTrace(setting.getSigningDetails()));
            java.util.Collections.sort(signatures); output.add(String.join(";", signatures));
            for (var list : java.util.List.of(settings.mPermissions.getPermissions(), settings.mPermissions.getPermissionTrees())) {
                var permissions = new java.util.ArrayList<String>();
                for (var permission : list) {
                    var info = permission.getPermissionInfo();
                    permissions.add(info.name + ":" + info.packageName + ":" + info.protectionLevel + ":" + info.icon + ":" + info.nonLocalizedLabel + ":" + permission.getType());
                }
                java.util.Collections.sort(permissions); output.add(String.join(";", permissions));
            }
            var keysets = new java.io.ByteArrayOutputStream();
            var xml = android.util.Xml.resolveSerializer(keysets); xml.startDocument(null, true); xml.startTag(null, "packages");
            settings.getKeySetManagerService().writeKeySetManagerServiceLPr(xml);
            xml.endTag(null, "packages"); xml.endDocument();
            java.nio.file.Files.write(new java.io.File(directory, "owner-default-keysets-" + index).toPath(), keysets.toByteArray());
            output.add(main.exists() + "," + reserve.exists());
            java.nio.file.Files.write(new java.io.File(directory, "owner-default-output-" + index).toPath(), String.join("|", output).getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private static com.android.server.pm.permission.LegacyPermission configuredPermission() {
        var info = new android.content.pm.PermissionInfo();
        info.name = "a"; info.packageName = "configured"; info.protectionLevel = 2;
        info.icon = 42; info.nonLocalizedLabel = "configured-label";
        return new com.android.server.pm.permission.LegacyPermission(info, 1, 1234, new int[] {1001, 1002});
    }
    private static String signatureTrace(android.content.pm.SigningDetails details) throws Exception {
        var current = details.getSignatures(); var past = details.getPastSigningCertificates();
        var entries = new java.util.ArrayList<String>();
        if (current != null) for (var cert : current) entries.add(hex(java.security.MessageDigest.getInstance("SHA-256").digest(cert.toByteArray())));
        String signatures = current == null ? "null" : String.join(",", entries); entries.clear();
        if (past != null) for (var cert : past) entries.add(hex(java.security.MessageDigest.getInstance("SHA-256").digest(cert.toByteArray())) + ":" + cert.getFlags());
        return details.getSignatureSchemeVersion() + ":" + signatures + ":" + (past == null ? "null" : String.join(",", entries));
    }
    private static void verifySignatureEvents(java.io.File directory) throws Exception {
        byte[] seed = java.nio.file.Files.readAllBytes(new java.io.File(directory, "signature-event-seed").toPath());
        byte[] retry = java.nio.file.Files.readAllBytes(new java.io.File(directory, "signature-event-retry").toPath());
        for (int index = 0; ; index++) {
            var input = new java.io.File(directory, "signature-event-input-" + index);
            if (!input.exists()) break;
            var owner = new PackageSignatures();
            var table = new java.util.ArrayList<android.content.pm.Signature>();
            readSignatureEvent(seed, owner, table);
            String first = readSignatureEvent(java.nio.file.Files.readAllBytes(input.toPath()), owner, table);
            String second = readSignatureEvent(retry, owner, table);
            java.nio.file.Files.write(new java.io.File(directory, "signature-event-output-" + index).toPath(),
                    (first + "\n" + second).getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private static String readSignatureEvent(byte[] bytes, PackageSignatures owner,
            java.util.ArrayList<android.content.pm.Signature> table) throws Exception {
        String status = "ok";
        try {
            var parser = android.util.Xml.resolvePullParser(new java.io.ByteArrayInputStream(bytes));
            int type;
            while ((type = parser.next()) != 2 && type != 1) {}
            if (type == 1) status = "absent";
            else owner.readXml(parser, table);
        } catch (Exception error) { status = "error"; }
        var flags = new java.util.ArrayList<String>();
        var current = owner.mSigningDetails.getSignatures();
        if (current != null) for (var cert : current) flags.add(Integer.toString(cert.getFlags()));
        var entries = new java.util.ArrayList<String>();
        for (var cert : table) entries.add(cert == null ? "null" : hex(java.security.MessageDigest.getInstance("SHA-256").digest(cert.toByteArray())) + ":" + cert.getFlags());
        return status + "|" + signatureTrace(owner.mSigningDetails) + "|" + (current == null ? "null" : String.join(",", flags)) + "|" + String.join(",", entries);
    }
    private static void verifySettingsDefaults(java.io.File directory) throws Exception {
        for (int index = 0; ; index++) {
            var input = new java.io.File(directory, "defaults-input-" + index);
            if (!input.exists()) break;
            var data = new java.io.File(directory, "defaults-original-" + index);
            var system = new java.io.File(data, "system"); system.mkdirs();
            var main = new java.io.File(system, "packages.xml");
            var reserve = new java.io.File(system, "packages.xml.reservecopy");
            java.nio.file.Files.write(main.toPath(), java.nio.file.Files.readAllBytes(input.toPath()));
            java.nio.file.Files.write(reserve.toPath(), "<packages/>".getBytes(java.nio.charset.StandardCharsets.UTF_8));
            var settings = new Settings(data, null, null, null, null, new PackageManagerTracedLock());
            boolean first = !settings.readSettingsLPw(null, java.util.List.of(), new android.util.ArrayMap<>());
            var active = new java.util.ArrayList<String>();
            for (var setting : settings.getPackagesLocked().values()) active.add(defaultSetting(setting, true));
            var disabled = new java.util.ArrayList<String>();
            for (var setting : settings.getDisabledSystemPackagesLocked().values()) disabled.add(defaultSetting(setting, false));
            var shared = new java.util.ArrayList<String>();
            for (var setting : settings.getAllSharedUsersLPw()) shared.add(setting.getName() + ":" + setting.mAppId + ":" + setting.getFlags());
            java.util.Collections.sort(active); java.util.Collections.sort(disabled); java.util.Collections.sort(shared);
            String output = first + "|" + String.join(";", active) + "|" + String.join(";", disabled) + "|" + String.join(";", shared) + "|" + main.exists() + "," + reserve.exists();
            java.nio.file.Files.write(new java.io.File(directory, "defaults-output-" + index).toPath(), output.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private static String defaultSetting(PackageSetting setting, boolean active) {
        var state = (com.android.server.pm.pkg.PackageState)setting;
        var fields = new java.util.ArrayList<String>();
        fields.add(setting.getPackageName()); fields.add(Integer.toString(setting.getAppId()));
        fields.add(Long.toString(setting.getVersionCode())); fields.add(Integer.toString(state.getTargetSdkVersion()));
        fields.add(Integer.toString(setting.getFlags())); fields.add(Integer.toString(setting.getPrivateFlags()));
        byte[] hash = state.getRestrictUpdateHash(); fields.add(hash == null ? "null" : hex(hash));
        fields.add(Boolean.toString(setting.isScannedAsStoppedSystemApp()));
        fields.add(Long.toString(state.getLastModifiedTime())); fields.add(Long.toString(state.getLastUpdateTime()));
        fields.add(Integer.toString(setting.getAppMetadataSource()));
        if (active) {
            var source = setting.getInstallSource();
            fields.add(Integer.toString(source.mInstallerPackageUid)); fields.add(Integer.toString(source.mPackageSource));
            fields.add(Boolean.toString(source.mIsOrphaned)); fields.add(Boolean.toString(source.mIsInitiatingPackageUninstalled));
            fields.add(Integer.toString(setting.getCategoryOverride()));
            fields.add(Boolean.toString(state.isUpdateAvailable())); fields.add(Boolean.toString(state.isForceQueryableOverride()));
            fields.add(Boolean.toString(state.isPendingRestore())); fields.add(Boolean.toString(state.isDebuggable()));
            fields.add(Integer.toString(setting.getBaseRevisionCode())); fields.add(Integer.toString(setting.getPageSizeAppCompatFlags()));
            fields.add(String.format("%08x", Float.floatToRawIntBits(setting.getLoadingProgress())));
            fields.add(Long.toString(setting.getLoadingCompletedTime()));
        }
        var libraries = new java.util.ArrayList<String>();
        String[] names = setting.getUsesStaticLibraries(); long[] versions = setting.getUsesStaticLibrariesVersions();
        for (int i = 0; i < names.length; i++) libraries.add(names[i] + ":" + versions[i]);
        fields.add(String.join("/", libraries)); libraries.clear();
        names = setting.getUsesSdkLibraries(); versions = setting.getUsesSdkLibrariesVersionsMajor(); boolean[] optional = setting.getUsesSdkLibrariesOptional();
        for (int i = 0; i < names.length; i++) libraries.add(names[i] + ":" + versions[i] + ":" + optional[i]);
        fields.add(String.join("/", libraries));
        if (active) {
            libraries.clear(); names = setting.getSplitNames(); int[] revisions = setting.getSplitRevisionCodes();
            for (int i = 0; i < names.length; i++) libraries.add(names[i] + ":" + revisions[i]);
            fields.add(String.join("/", libraries));
        }
        return String.join(",", fields);
    }
    private static void verifySettingsVersionRecovery(java.io.File directory) throws Exception {
        for (int index = 0; ; index++) {
            var input = new java.io.File(directory, "version-input-" + index);
            if (!input.exists()) break;
            var data = new java.io.File(directory, "version-original-" + index);
            var system = new java.io.File(data, "system"); system.mkdirs();
            var main = new java.io.File(system, "packages.xml");
            var reserve = new java.io.File(system, "packages.xml.reservecopy");
            java.nio.file.Files.write(main.toPath(), java.nio.file.Files.readAllBytes(input.toPath()));
            java.nio.file.Files.write(reserve.toPath(), "<packages/>".getBytes(java.nio.charset.StandardCharsets.UTF_8));
            // Only the settings-read owner runs: these cases do not call runtime
            // permissions, handlers or domain verification dependencies.
            var settings = new Settings(data, null, null, null, null, new PackageManagerTracedLock());
            var seeded = settings.findOrCreateVersion("v");
            seeded.sdkVersion = 35; seeded.databaseVersion = 7;
            seeded.buildFingerprint = "old"; seeded.fingerprint = "old-partitions";
            boolean first = !settings.readSettingsLPw(null, java.util.List.of(), new android.util.ArrayMap<>());
            var output = new java.util.ArrayList<String>(); output.add(Boolean.toString(first));
            for (String uuid : new String[]{"v", "other", null, "primary_physical"}) {
                var version = settings.findOrCreateVersion(uuid);
                output.add(version.sdkVersion + "," + version.databaseVersion + "," + version.buildFingerprint + "," + version.fingerprint);
            }
            output.add(main.exists() + "," + reserve.exists());
            java.nio.file.Files.write(new java.io.File(directory, "version-output-" + index).toPath(),
                String.join("|", output).getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private static void verifyPullMatrix(java.io.File directory) throws Exception {
        for (int index = 0; ; index++) {
            var input = new java.io.File(directory, "pull-input-" + index);
            if (!input.exists()) break;
            var events = new java.util.ArrayList<String>(); boolean started = false;
            try (var stream = new java.io.FileInputStream(input)) {
                var parser = android.util.Xml.resolvePullParser(stream);
                while (true) {
                    int event = parser.next();
                    if (event == 2) {
                        started = true; var attrs = new java.util.ArrayList<String>();
                        for (String name : new String[]{"name", "codePath", "sdkVersion"}) {
                            String value = parser.getAttributeValue(null, name);
                            attrs.add(value == null ? "missing" : hex(value.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
                        }
                        events.add("start:" + parser.getName() + ":" + parser.getDepth() + ":" + String.join(",", attrs));
                    } else if (event == 3) {
                        events.add("end:" + parser.getName() + ":" + parser.getDepth());
                        if (started && parser.getDepth() == 1) break;
                    } else if (event == 4) {
                        events.add("text:" + parser.getDepth() + ":" + hex(parser.getText().getBytes(java.nio.charset.StandardCharsets.UTF_8)));
                    } else if (event == 1) { events.add("end-document"); break; }
                }
            } catch (Exception failure) { events.add("error"); }
            java.nio.file.Files.write(new java.io.File(directory, "pull-output-" + index).toPath(),
                String.join("|", events).getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private static void verifyRecoveryMatrix(java.io.File directory) throws Exception {
        for (int index = 0; ; index++) {
            var inputFile = new java.io.File(directory, "recovery-input-" + index);
            if (!inputFile.exists()) break;
            var inputs = new java.util.Properties();
            try (var input = new java.io.FileInputStream(inputFile)) { inputs.load(input); }
            var root = new java.io.File(directory, "recovery-original-" + index); root.mkdirs();
            var files = new java.io.File[]{new java.io.File(root, "main"), new java.io.File(root, "backup"), new java.io.File(root, "reserve")};
            for (int slot = 0; slot < 3; slot++) {
                files[slot].delete(); String value = inputs.getProperty(Integer.toString(slot));
                if (!value.equals("missing")) {
                    byte[] bytes = new byte[value.length() / 2];
                    for (int i = 0; i < bytes.length; i++) bytes[i] = (byte)Integer.parseInt(value.substring(i * 2, i * 2 + 2), 16);
                    java.nio.file.Files.write(files[slot].toPath(), bytes);
                }
            }
            var events = new java.util.ArrayList<String>(); boolean failed = false, first = false;
            var sources = new String[]{"Main", "Backup", "Reserve"};
            try (var atomic = new ResilientAtomicFile(files[0], files[1], files[2], 0660, "fixture", null)) {
                while (true) {
                    int selected = files[1].exists() ? 1 : files[0].exists() ? 0 : files[2].exists() ? 2 : -1;
                    boolean main = files[0].exists(), reserve = files[2].exists();
                    var stream = atomic.openRead();
                    if (stream == null) { events.add("absent"); first = !failed; break; }
                    events.add("selected." + sources[selected]);
                    if (selected == 1) {
                        if (main && !files[0].exists()) events.add("removed.Main");
                        if (reserve && !files[2].exists()) events.add("removed.Reserve");
                    }
                    try {
                        var parser = android.util.Xml.resolvePullParser(stream); int event;
                        do { event = parser.next(); } while (event != 1 && event != 2);
                        if (event == 1) { events.add("no-root." + sources[selected]); first = !failed; break; }
                        while (parser.next() != 1) {}
                        first = false; stream.close(); break;
                    } catch (Exception failure) {
                        failed = true; events.add("failed." + sources[selected]);
                        atomic.failRead(stream, failure); events.add("removed." + sources[selected]);
                    }
                }
            }
            var remains = new java.util.ArrayList<String>();
            for (var file : files) remains.add(file.exists() ? hex(java.nio.file.Files.readAllBytes(file.toPath())) : "missing");
            String output = first + "|" + String.join(",", events) + "|" + String.join(";", remains);
            java.nio.file.Files.write(new java.io.File(directory, "recovery-output-" + index).toPath(), output.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
    private static void verifyEmptyDocuments(java.io.File directory) throws Exception {
        var results = new java.util.ArrayList<String>();
        byte[][] inputs = {new byte[0], " \n".getBytes(java.nio.charset.StandardCharsets.UTF_8),
            new byte[]{65, 66, 88, 0}, new byte[]{65, 66, 88, 0, 16, 17}};
        for (int i = 0; i < inputs.length; i++) {
            try {
                var parser = android.util.Xml.resolvePullParser(new java.io.ByteArrayInputStream(inputs[i]));
                int event; do { event = parser.next(); } while (event != 1 && event != 2);
                results.add(i + "=" + event);
            } catch (Exception failure) { results.add(i + "=" + failure.getClass().getName()); }
        }
        java.nio.file.Files.write(new java.io.File(directory, "empty-document-original").toPath(), results);
    }
    private static void verifyFirstWriteRetry(java.io.File directory) throws Exception {
        var root = new java.io.File(directory, "original-first-write-retry");
        var main = new java.io.File(root, "main");
        var backup = new java.io.File(root, "backup");
        var reserve = new java.io.File(root, "reserve");
        // Each invocation creates fresh files after the prior successful retry.
        main.delete(); backup.delete(); reserve.delete();
        try (var atomic = new ResilientAtomicFile(main, backup, reserve, 0660, "fixture", null)) {
            var stream = atomic.startWrite(); stream.write(new byte[]{1, 2, 3});
            atomic.failWrite(stream);
            if (main.exists() || backup.exists() || !reserve.exists() || reserve.length() != 0)
                throw new AssertionError("original first-write failure artifacts");
            stream = atomic.startWrite(); stream.write(new byte[]{4, 5, 6});
            atomic.finishWrite(stream, false);
            if (backup.exists() || !java.util.Arrays.equals(java.nio.file.Files.readAllBytes(main.toPath()), new byte[]{4, 5, 6})
                    || !java.util.Arrays.equals(java.nio.file.Files.readAllBytes(main.toPath()), java.nio.file.Files.readAllBytes(reserve.toPath())))
                throw new AssertionError("original first-write retry output");
        }
        main.delete(); reserve.delete();
        if (!reserve.mkdir()) throw new AssertionError("reserve failure directory");
        var sentinel = new java.io.File(reserve, "keep"); java.nio.file.Files.write(sentinel.toPath(), new byte[]{1});
        try (var atomic = new ResilientAtomicFile(main, backup, reserve, 0660, "fixture", null)) {
            try { atomic.startWrite(); throw new AssertionError("original reserve open failure accepted"); }
            catch (java.io.IOException expected) {}
            if (!main.exists() || main.length() != 0 || backup.exists())
                throw new AssertionError("original failed-start main ownership");
            boolean sentinelDeleted = sentinel.delete();
            boolean reserveDeleted = reserve.delete();
            if (!sentinelDeleted || !reserveDeleted) throw new AssertionError("owned reserve failure cleanup: sentinel=" + sentinelDeleted
                + " reserve=" + reserveDeleted + " children=" + java.util.Arrays.toString(reserve.list()));
            var stream = atomic.startWrite(); stream.write(new byte[]{7}); atomic.finishWrite(stream, false);
            if (backup.exists() || !java.util.Arrays.equals(java.nio.file.Files.readAllBytes(main.toPath()), new byte[]{7})
                    || !java.util.Arrays.equals(java.nio.file.Files.readAllBytes(main.toPath()), java.nio.file.Files.readAllBytes(reserve.toPath())))
                throw new AssertionError("original failed-start retry output");
        }
    }
    private static void writeInventory(java.io.File directory,
            dev.aim.server.PackageScanLease lease) throws Exception {
        var packages = new android.util.ArrayMap<String, PackageSetting>();
        for (String filename : java.nio.file.Files.readAllLines(new java.io.File(directory, "settings-owner-order").toPath())) {
            var parcel = android.os.Parcel.obtain(); PackageSetting setting;
            try {
                byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, filename).toPath());
                parcel.unmarshall(bytes, 0, bytes.length); parcel.setDataPosition(0);
                setting = CapturedPackageSetting.from(dev.aim.server.PackageSettingData.read(parcel), 1, false);
                if (parcel.dataAvail() != 0) throw new AssertionError("inventory setting tail");
            } finally { parcel.recycle(); }
            var codeFile = new java.io.File(directory, filename.substring(0, filename.length() - ".writer-setting".length()));
            var code = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(java.nio.file.Files.readAllBytes(codeFile.toPath()));
            setting.setPkg(code);
            // SigningDetails on parsed code and on saved PackageSetting are
            // distinct owners; persistence writes the latter's lineage flags.
            var signingParcel = android.os.Parcel.obtain();
            try {
                byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(codeFile.getPath() + ".saved-signing").toPath());
                signingParcel.unmarshall(bytes, 0, bytes.length); signingParcel.setDataPosition(0);
                var signing = dev.aim.server.PackageSigningState.CREATOR.createFromParcel(signingParcel);
                if (signingParcel.dataAvail() != 0 || !setting.getPackageName().equals(signing.getPackageName()))
                    throw new AssertionError("inventory saved signer owner differs");
                setting.setSigningDetails(signing.getPackageSigningDetails());
            } finally { signingParcel.recycle(); }
            packages.put(setting.getPackageName(), setting);
        }
        var settings = new Settings(java.util.Map.of());
        var certificates = new java.util.ArrayList<android.content.pm.Signature>();
        try (var output = new java.io.FileOutputStream(new java.io.File(directory, "settings-inventory-original"))) {
            var xml = android.util.Xml.resolveSerializer(output);
            xml.startDocument(null, true); xml.startTag(null, "packages");
            for (var setting : packages.values()) settings.writePackageLPr(xml, certificates, setting);
            var groups = new android.util.ArrayMap<String, dev.aim.server.SharedUserData>();
            for (String name : lease.getSharedUserNames()) groups.put(name, lease.getSharedUserData(name));
            for (var entry : groups.entrySet()) {
                var sigs = new PackageSignatures(); sigs.mSigningDetails = entry.getValue().getSigningDetails();
                xml.startTag(null, "shared-user"); xml.attribute(null, "name", entry.getKey());
                xml.attributeInt(null, "userId", entry.getValue().getAppId());
                sigs.writeXml(xml, "sigs", certificates); xml.endTag(null, "shared-user");
            }
            xml.endTag(null, "packages"); xml.endDocument();
        }
    }
    private static void verifyArrayMapOrder() {
        var map = new android.util.ArrayMap<String, Integer>();
        map.put("BB", 1); map.put("Aa", 2);
        if (!new java.util.ArrayList<>(map.keySet()).get(0).equals("BB") || !new java.util.ArrayList<>(map.keySet()).get(1).equals("Aa"))
            throw new AssertionError("original ArrayMap collision insertion order");
        map.put("z", 0); map.put("negative.hash.owner", 5);
        if (!new java.util.ArrayList<>(map.keySet()).equals(java.util.List.of("negative.hash.owner", "z", "BB", "Aa")))
            throw new AssertionError("original ArrayMap signed hash order");
        map.put("BB", 3);
        if (!new java.util.ArrayList<>(map.keySet()).equals(java.util.List.of("negative.hash.owner", "z", "BB", "Aa")))
            throw new AssertionError("original ArrayMap lookup order");
        map.remove("BB"); map.put("BB", 4);
        if (!new java.util.ArrayList<>(map.keySet()).equals(java.util.List.of("negative.hash.owner", "z", "Aa", "BB")))
            throw new AssertionError("original ArrayMap collision recreation order");
    }
    private static void verifyNativeSignatures(java.io.File directory,
            dev.aim.server.PackageScanLease lease, PackageSetting setting) throws Exception {
        var certificates = new java.util.ArrayList<android.content.pm.Signature>();
        try (var input = new java.io.FileInputStream(new java.io.File(directory, "native-settings-writer/system/packages.xml"))) {
            var parser = android.util.Xml.resolvePullParser(input);
            int event;
            while ((event = parser.next()) != 1) {
                if (event != 2 || !(parser.getName().equals("package") || parser.getName().equals("shared-user"))) continue;
                String owner = parser.getName(), name = parser.getAttributeValue(null, "name");
                int depth = parser.getDepth();
                while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
                    if (event != 2) continue;
                    String tag = parser.getName();
                    if (!(tag.equals("sigs") || tag.equals("install-initiator-sigs"))) {
                        int skipped = parser.getDepth();
                        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > skipped)) {}
                        continue;
                    }
                    var actual = new PackageSignatures(); actual.readXml(parser, certificates);
                    android.content.pm.SigningDetails expected = null;
                    if (owner.equals("shared-user")) expected = lease.getSharedUserData(name).getSigningDetails();
                    else if (name.equals(setting.getPackageName())) {
                        expected = tag.equals("sigs") ? setting.getSigningDetails()
                            : setting.getInstallSource().mInitiatingPackageSignatures.mSigningDetails;
                    }
                    if (expected != null) compareSignatures(actual.mSigningDetails, expected, name + "/" + tag);
                }
            }
        }
    }
    private static void compareSignatures(android.content.pm.SigningDetails actual,
            android.content.pm.SigningDetails expected, String owner) {
        if (actual.getSignatureSchemeVersion() != expected.getSignatureSchemeVersion()
                || !java.util.Arrays.equals(actual.getSignatures(), expected.getSignatures())
                || !java.util.Arrays.equals(actual.getPastSigningCertificates(), expected.getPastSigningCertificates()))
            throw new AssertionError("original certificate table restoration differs: " + owner);
        var past = actual.getPastSigningCertificates();
        if (past != null) for (int i = 0; i < past.length; i++) {
            if (past[i].getFlags() != expected.getPastSigningCertificates()[i].getFlags())
                throw new AssertionError("original past certificate capability differs: " + owner);
        }
    }

    private static void verifyReindexedCertificates(java.io.File directory) throws Exception {
        var expected = new java.util.Properties();
        try (var input = new java.io.FileInputStream(new java.io.File(directory, "reindexed-certificates.properties"))) { expected.load(input); }
        var certificates = new java.util.ArrayList<android.content.pm.Signature>();
        int count = 0;
        try (var input = new java.io.FileInputStream(new java.io.File(directory, "reindexed-settings-writer/system/packages.xml"))) {
            var parser = android.util.Xml.resolvePullParser(input); int event;
            while ((event = parser.next()) != 1) {
                if (event != 2 || !(parser.getName().equals("package") || parser.getName().equals("shared-user"))) continue;
                String owner = parser.getName(), name = parser.getAttributeValue(null, "name"); int depth = parser.getDepth();
                while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
                    if (event != 2) continue;
                    String tag = parser.getName();
                    if (!(tag.equals("sigs") || tag.equals("install-initiator-sigs"))) {
                        int skipped = parser.getDepth();
                        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > skipped)) {}
                        continue;
                    }
                    var actual = new PackageSignatures(); actual.readXml(parser, certificates);
                    String key = owner + "/" + name + "/" + tag;
                    if (!describe(actual.mSigningDetails).equals(expected.getProperty(key)))
                        throw new AssertionError("reindexed original certificate table differs: " + key);
                    count++;
                }
            }
        }
        if (count != expected.size()) throw new AssertionError("reindexed original signature inventory differs");
    }
    private static String describe(android.content.pm.SigningDetails details) {
        var current = new java.util.ArrayList<String>();
        for (var signature : details.getSignatures()) current.add(hex(signature.toByteArray()));
        var past = details.getPastSigningCertificates(); String history = "null";
        if (past != null) {
            var values = new java.util.ArrayList<String>();
            for (var signature : past) values.add(hex(signature.toByteArray()) + ":" + signature.getFlags());
            history = String.join(",", values);
        }
        return details.getSignatureSchemeVersion() + "|" + String.join(",", current) + "|" + history;
    }
    private static String hex(byte[] bytes) {
        var text = new StringBuilder();
        for (byte value : bytes) text.append(String.format("%02x", value & 255));
        return text.toString();
    }

}
