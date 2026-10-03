package com.android.server.pm;

public final class SettingRemovalOracle {
    private static PackageSetting pkg(String name, int id, boolean shared) {
        var p = new PackageSetting(name, null, new java.io.File("/data/app/" + name), 1, 0,
            java.util.UUID.randomUUID()).setAppId(id);
        if (shared) p.setSharedUserAppId(id);
        return p;
    }
    private static String hex(byte[] bytes) {
        var text = new StringBuilder();
        for (byte b : bytes) {
            text.append(Character.forDigit((b & 255) >>> 4, 16));
            text.append(Character.forDigit(b & 15, 16));
        }
        return text.toString();
    }
    public static void main(String[] args) throws Exception {
        for (int kind = 0; kind < 3; kind++) {
            boolean shared = kind != 0;
            var a = pkg("a", 10100, shared);
            var b = pkg("b", shared ? 10100 : 10101, shared);
            var settings = new Settings(java.util.Map.of("a", a, "b", b));
            if (shared) {
                var group = settings.addSharedUserLPw("group", 10100, 0, 0);
                group.addPackage(a);
                group.addPackage(b);
                if (kind == 2) a.setPkg((com.android.server.pm.pkg.AndroidPackage)
                    com.android.internal.pm.parsing.pkg.PackageImpl.forTesting("a"));
                if (kind == 2 && !settings.disableSystemPackageLPw("a", false)) {
                    throw new AssertionError("factory reservation was not created");
                }
            } else {
                settings.registerAppIdLPw(a, false);
                settings.registerAppIdLPw(b, false);
            }
            for (String name : new String[] {"a", "b", "missing"}) {
                System.out.println(kind + " " + name + " " + settings.removePackageAndAppIdLPw(name)
                    + " " + (settings.getSettingLPr(10100) != null));
            }
        }
        var store = pkg("store", 10100, false);
        var app = pkg("app", 10101, false);
        var source = InstallSource.create("store", "store", "store", 10100, "store", "tag", 2, false, false);
        app.setInstallSource(source);
        var settings = new Settings(java.util.Map.of("store", store, "app", app));
        settings.registerAppIdLPw(store, false);
        settings.registerAppIdLPw(app, false);
        settings.addInstallerPackageNames(source);
        settings.removePackageAndAppIdLPw("store");
        source = app.getInstallSource();
        System.out.println("source " + source.mInitiatingPackageName + " " + source.mIsInitiatingPackageUninstalled
            + " " + source.mOriginatingPackageName + " " + source.mInstallerPackageName
            + " " + source.mInstallerPackageUid + " " + source.mUpdateOwnerPackageName
            + " " + source.mInstallerAttributionTag + " " + source.mIsOrphaned + " " + source.mPackageSource);
        if (args.length != 0) {
            var certificates = new java.util.ArrayList<android.content.pm.Signature>();
            try (var input = new java.io.FileInputStream(args[0])) {
                var parser = android.util.Xml.resolvePullParser(input);
                int type;
                while ((type = parser.next()) != 1) {
                    if (type == 2 && parser.getDepth() == 3
                            && ("sigs".equals(parser.getName())
                                || "install-initiator-sigs".equals(parser.getName()))) {
                        String tag = parser.getName();
                        var signatures = new PackageSignatures();
                        signatures.readXml(parser, certificates);
                        var details = signatures.mSigningDetails;
                        System.out.println("persist " + tag + " " + hex(details.getSignatures()[0].toByteArray())
                            + " " + (details.getPastSigningCertificates() == null ? "-"
                                : hex(details.getPastSigningCertificates()[0].toByteArray()) + ":"
                                    + details.getPastSigningCertificates()[0].getFlags()));
                    }
                }
            }
        }
        settings.addRenamedPackageLPw("real", "internal");
        settings.addRenamedPackageLPw("other", "internal");
        for (String name : new String[] {"missing", null, "real", "real"}) {
            settings.removeRenamedPackageLPw(name);
            System.out.println("rename " + settings.getRenamedPackageLPr("real") + " "
                + settings.getRenamedPackageLPr("other"));
        }
        if (args.length > 1) {
            var real = new java.io.File(args[1]);
            var temp = new java.io.File(args[1] + ".tmp");
            var journal = new com.android.internal.util.JournaledFile(real, temp);
            try (var output = new java.io.FileOutputStream(temp)) { output.write("stale".getBytes()); }
            String original = new String(java.nio.file.Files.readAllBytes(journal.chooseForRead().toPath()), java.nio.charset.StandardCharsets.UTF_8);
            System.out.println("journal read " + original.trim() + " " + temp.exists());
            try (var output = new java.io.FileOutputStream(journal.chooseForWrite())) { output.write("partial".getBytes()); }
            journal.rollback();
            System.out.println("journal rollback " + new String(java.nio.file.Files.readAllBytes(real.toPath()), java.nio.charset.StandardCharsets.UTF_8).trim() + " " + temp.exists());
            try (var output = new java.io.FileOutputStream(journal.chooseForWrite())) { output.write(original.getBytes(java.nio.charset.StandardCharsets.UTF_8)); }
            journal.commit();
            System.out.println("journal commit " + new String(java.nio.file.Files.readAllBytes(real.toPath()), java.nio.charset.StandardCharsets.UTF_8).trim() + " " + temp.exists());
        }
        // The test-only Settings constructor starts BackgroundThread.
        System.exit(0);
    }
}
