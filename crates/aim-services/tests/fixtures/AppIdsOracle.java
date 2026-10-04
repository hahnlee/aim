package com.android.server.pm;

public final class AppIdsOracle {
    public static void main(String[] args) throws Exception {
        if (args.length != 0 && args[0].equals("read-store-signatures")) {
            var table = new java.util.ArrayList<android.content.pm.Signature>();
            try (var in = new java.io.FileInputStream(args[1])) {
                var parser = android.util.Xml.resolvePullParser(in);
                int event;
                while ((event = parser.next()) != 1) {
                    if (event != 2 || !(parser.getName().equals("package") || parser.getName().equals("shared-user"))) continue;
                    String owner = parser.getName(), name = parser.getAttributeValue(null, "name");
                    int depth = parser.getDepth();
                    while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
                        if (event != 2) continue;
                        String tag = parser.getName();
                        if (tag.equals("sigs") || tag.equals("install-initiator-sigs")) {
                            var signing = new PackageSignatures(); signing.readXml(parser, table);
                            var current = signing.mSigningDetails.getSignatures();
                            var flags = new java.util.ArrayList<String>();
                            if (current != null) for (var cert : current) flags.add(Integer.toString(cert.getFlags()));
                            System.out.println(owner + ":" + name + ":" + tag + ":" + (current == null ? "null" : String.join(",", flags)));
                        } else {
                            int skipped = parser.getDepth();
                            while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > skipped)) {}
                        }
                    }
                }
            }
            return;
        }
        if (args.length != 0 && args[0].equals("read-signatures")) {
            java.util.ArrayList<android.content.pm.Signature> certificates = new java.util.ArrayList<>();
            try (java.io.InputStream input = new java.io.FileInputStream(args[1])) {
                com.android.modules.utils.TypedXmlPullParser parser = android.util.Xml.resolvePullParser(input);
                String owner = null;
                String name = null;
                int type;
                while ((type = parser.next()) != 1) {
                    int depth = parser.getDepth();
                    if (type == 2 && depth == 2) {
                        String tag = parser.getName();
                        if (tag.equals("package") || tag.equals("updated-package") || tag.equals("shared-user")) {
                            owner = tag;
                            name = parser.getAttributeValue(null, "name");
                        }
                    } else if (type == 2 && depth == 3 && owner != null && parser.getName().equals("sigs")) {
                        PackageSignatures signatures = new PackageSignatures();
                        signatures.readXml(parser, certificates);
                        android.content.pm.SigningDetails details = signatures.mSigningDetails;
                        if (details.getSignatures() == null) throw new AssertionError("original rejected " + name);
                        System.out.println(owner + " " + name + " " + details.getSignatureSchemeVersion()
                            + " " + certificateText(details.getSignatures(), false)
                            + " " + certificateText(details.getPastSigningCertificates(), true)
                            + " " + details.getPublicKeys().size());
                    } else if (type == 3 && depth == 2) {
                        owner = null;
                        name = null;
                    }
                }
            }
            return;
        }
        if (args.length != 0 && (args[0].equals("merge") || args[0].equals("group-merge"))) {
            byte[][] certs = new byte[4][];
            for (int c = 0; c < certs.length; c++) {
                certs[c] = java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[c + 1]));
            }
            int[][] current = {{}, {1}, {2}, {2}, {3}, {3}, {3}, {4}, {1,2}, {2,1}, {1,3}, {3}, {2}};
            int[][] past = {null, null, null, {1,2}, {1,2,3}, {2,3}, {4,2,3}, {1,2,4}, null, null, null, {3}, {1,2}};
            int[][] flags = {null, null, null, {3,0}, {0,2,0}, {8,0}, {3,2,0}, {3,2,0}, null, null, null, {0}, {0,8}};
            android.content.pm.SigningDetails[] cases = new android.content.pm.SigningDetails[current.length];
            for (int i = 0; i < cases.length; i++) {
                if (current[i].length == 0) { cases[i] = android.content.pm.SigningDetails.UNKNOWN; continue; }
                android.content.pm.Signature[] signers = realSignatures(current[i], null, certs);
                for (var signer : signers) signer.setFlags(i + 1);
                android.content.pm.Signature[] lineage = realSignatures(past[i], flags[i], certs);
                cases[i] = new android.content.pm.SigningDetails(signers, i % 4 + 1, lineage);
            }
            if (args[0].equals("group-merge")) {
                for (int i = 0; i < cases.length; i++) {
                    for (int j = 0; j < cases.length; j++) {
                        for (int k = 0; k < cases.length; k++) {
                            android.content.pm.SigningDetails merged = cases[i].mergeLineageWith(cases[j], 1);
                            boolean changed = merged != cases[i];
                            if (changed) merged = merged.mergeLineageWith(cases[k], 2);
                            System.out.println(i + " " + j + " " + k + " " + changed + " "
                                + merged.getSignatureSchemeVersion() + " " + describe(merged.getSignatures(), certs, true)
                                + " " + describe(merged.getPastSigningCertificates(), certs, true)
                                + " " + (merged.getPublicKeys() == null ? 0 : merged.getPublicKeys().size()));
                        }
                    }
                }
                return;
            }
            for (int i = 0; i < cases.length; i++) {
                for (int j = 0; j < cases.length; j++) {
                    for (int rule = 0; rule < 3; rule++) {
                        android.content.pm.SigningDetails merged = cases[i].mergeLineageWith(cases[j], rule);
                        System.out.println(i + " " + j + " " + rule + " " + (merged == cases[i]) + " "
                            + merged.getSignatureSchemeVersion() + " " + describe(merged.getSignatures(), certs, true)
                            + " " + describe(merged.getPastSigningCertificates(), certs, true)
                            + " " + (merged.getPublicKeys() == null ? 0 : merged.getPublicKeys().size()));
                    }
                }
            }
            return;
        }
        if (args.length != 0 && (args[0].equals("trust") || args[0].equals("join") || args[0].equals("ancestry"))) {
            android.content.pm.SigningDetails[] cases = {
                android.content.pm.SigningDetails.UNKNOWN,
                details(new int[]{1}, null, null),
                details(new int[]{2}, null, null),
                details(new int[]{2}, new int[]{1, 2}, new int[]{1, 0}),
                details(new int[]{2}, new int[]{1, 2}, new int[]{0, 0}),
                details(new int[]{3}, new int[]{1, 2, 3}, new int[]{3, 8, 0}),
                details(new int[]{1, 2}, null, null),
                details(new int[]{2, 1}, null, null),
                details(new int[]{1, 3}, null, null),
            };
            if (args[0].equals("ancestry")) {
                cases = new android.content.pm.SigningDetails[]{
                    android.content.pm.SigningDetails.UNKNOWN,
                    details(new int[]{1}, null, null),
                    details(new int[]{2}, null, null),
                    details(new int[]{2}, new int[]{1, 2}, new int[]{3, 0}),
                    details(new int[]{3}, new int[]{1, 2, 3}, new int[]{0, 2, 0}),
                    details(new int[]{3}, new int[]{2, 3}, new int[]{8, 0}),
                    details(new int[]{3}, new int[]{4, 2, 3}, new int[]{3, 2, 0}),
                    details(new int[]{4}, new int[]{1, 2, 4}, new int[]{3, 2, 0}),
                    details(new int[]{1, 2}, null, null),
                    details(new int[]{2, 1}, null, null),
                    details(new int[]{1, 3}, null, null),
                    details(new int[]{3}, new int[]{3}, new int[]{0}),
                };
                for (int i = 0; i < cases.length; i++) {
                    for (int j = 0; j < cases.length; j++) {
                        System.out.println(i + " " + j + " " + cases[i].hasCommonAncestor(cases[j]));
                    }
                }
                return;
            }
            if (args[0].equals("join")) {
                for (int i = 0; i < cases.length; i++) {
                    for (int j = 0; j < cases.length; j++) {
                        for (int k = 0; k < cases.length; k++) {
                            SharedUserSetting group = new SharedUserSetting("group", 1, 8);
                            group.signatures.mSigningDetails = cases[j];
                            PackageSetting member = new PackageSetting("member", null,
                                new java.io.File("/data/app/member"), 0, 0,
                                new java.util.UUID(0, 1)).setSigningDetails(cases[k]);
                            group.addPackage(member);
                            for (int kind = 0; kind < 3; kind++) {
                                System.out.println(i + " " + j + " " + k + " " + kind + " "
                                    + PackageManagerServiceUtils.canJoinSharedUserId("candidate", cases[i], group, kind));
                            }
                        }
                    }
                }
                return;
            }
            for (int i = 0; i < cases.length; i++) {
                for (int j = 0; j < cases.length; j++) {
                    android.content.pm.SigningDetails candidate = cases[i], old = cases[j];
                    boolean update = candidate.checkCapability(old, 1) || old.checkCapability(candidate, 8);
                    for (int flag : new int[]{0, 1, 2, 3, 8, 31}) {
                        System.out.println(i + " " + j + " " + flag + " "
                            + candidate.checkCapability(old, flag) + " " + candidate.hasAncestor(old)
                            + " " + candidate.hasAncestorOrSelf(old) + " " + update + " "
                            + (update || old.hasAncestorOrSelf(candidate)));
                    }
                }
            }
            return;
        }
        if (args.length != 0 && args[0].equals("identity")) {
            com.android.internal.pm.parsing.pkg.PackageImpl pkg =
                (com.android.internal.pm.parsing.pkg.PackageImpl)
                    com.android.internal.pm.parsing.pkg.PackageImpl.forTesting("new");
            com.android.internal.pm.pkg.component.ParsedComponentImpl[] components = {
                new com.android.internal.pm.pkg.component.ParsedActivityImpl(),
                new com.android.internal.pm.pkg.component.ParsedActivityImpl(),
                new com.android.internal.pm.pkg.component.ParsedServiceImpl(),
                new com.android.internal.pm.pkg.component.ParsedProviderImpl(),
                new com.android.internal.pm.pkg.component.ParsedPermissionImpl(),
                new com.android.internal.pm.pkg.component.ParsedPermissionGroupImpl(),
                new com.android.internal.pm.pkg.component.ParsedInstrumentationImpl(),
            };
            for (com.android.internal.pm.pkg.component.ParsedComponentImpl c : components) {
                c.setName("new.Class");
                c.setPackageName("new");
                c.getComponentName(); // Populate the original cached ComponentName.
                if (c instanceof com.android.internal.pm.pkg.component.ParsedMainComponentImpl) {
                    ((com.android.internal.pm.pkg.component.ParsedMainComponentImpl) c).setProcessName("new:process");
                }
            }
            pkg.addActivity((com.android.internal.pm.pkg.component.ParsedActivity) components[0]);
            pkg.addReceiver((com.android.internal.pm.pkg.component.ParsedActivity) components[1]);
            pkg.addService((com.android.internal.pm.pkg.component.ParsedService) components[2]);
            pkg.addProvider((com.android.internal.pm.pkg.component.ParsedProvider) components[3]);
            pkg.addPermission((com.android.internal.pm.pkg.component.ParsedPermission) components[4]);
            pkg.addPermissionGroup((com.android.internal.pm.pkg.component.ParsedPermissionGroup) components[5]);
            pkg.addInstrumentation((com.android.internal.pm.pkg.component.ParsedInstrumentation) components[6]);
            pkg.setPackageName("old");
            System.out.println(pkg.getPackageName() + " " + pkg.getManifestPackageName());
            for (com.android.internal.pm.pkg.component.ParsedComponentImpl c : components) {
                String process = c instanceof com.android.internal.pm.pkg.component.ParsedMainComponentImpl
                    ? ((com.android.internal.pm.pkg.component.ParsedMainComponentImpl) c).getProcessName() : "-";
                System.out.println(c.getPackageName() + " " + c.getName() + " " + process
                    + " " + c.getComponentName().getPackageName() + " " + c.getComponentName().getClassName());
            }
            return;
        }
        if (args.length != 0 && args[0].equals("oem")) {
            com.android.server.SystemConfig config = new com.android.server.SystemConfig(false);
            for (int i = 1; i < args.length; i++) {
                config.readPermissions(android.util.Xml.newPullParser(), new java.io.File(args[i]), 0);
            }
            for (java.util.Map.Entry<String, Integer> entry : config.getOemDefinedUids().entrySet()) {
                System.out.println(entry.getKey() + " " + entry.getValue());
            }
            return;
        }
        AppIdSettingMap ids = new AppIdSettingMap();
        SettingBase a = new SharedUserSetting("a", 1, 8);
        SettingBase b = new SharedUserSetting("b", 1, 8);
        System.out.println(ids.registerExistingAppId(10002, a, "a"));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        ids.removeSetting(10001);
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        ids.replaceSetting(10001, a);
        System.out.println(ids.getSetting(10001) == a);
        ids.replaceSetting(1000, a);
        System.out.println(ids.getSetting(1000) == a);
        ids.removeSetting(10010);
        System.out.println(ids.acquireAndRegisterNewAppId(b));

        AppIdSettingMap restart = new AppIdSettingMap();
        restart.registerExistingAppId(10002, a, "a");
        System.out.println(restart.acquireAndRegisterNewAppId(b));
        AppIdSettingMap full = new AppIdSettingMap();
        int last = -1;
        for (int i = 0; i < 10000; i++) last = full.acquireAndRegisterNewAppId(a);
        System.out.println(last);
        System.out.println(full.acquireAndRegisterNewAppId(b));
        full.removeSetting(19999);
        System.out.println(full.acquireAndRegisterNewAppId(b));
    }
    private static String certificateText(android.content.pm.Signature[] signatures, boolean flags) {
        if (signatures == null) return "-";
        StringBuilder result = new StringBuilder();
        char[] hex = "0123456789abcdef".toCharArray();
        for (android.content.pm.Signature signature : signatures) {
            if (result.length() != 0) result.append(',');
            for (byte b : signature.toByteArray()) {
                result.append(hex[(b & 255) >>> 4]).append(hex[b & 15]);
            }
            if (flags) result.append(':').append(signature.getFlags());
        }
        return result.toString();
    }
    private static android.content.pm.Signature[] realSignatures(int[] ids, int[] flags, byte[][] certs) {
        if (ids == null) return null;
        android.content.pm.Signature[] result = new android.content.pm.Signature[ids.length];
        for (int i = 0; i < ids.length; i++) {
            result[i] = new android.content.pm.Signature(certs[ids[i] - 1]);
            if (flags != null) result[i].setFlags(flags[i]);
        }
        return result;
    }
    private static String describe(android.content.pm.Signature[] signatures, byte[][] certs, boolean flags) {
        if (signatures == null) return "-";
        String result = "";
        for (android.content.pm.Signature signature : signatures) {
            int id = -1;
            for (int i = 0; i < certs.length; i++) {
                if (java.util.Arrays.equals(signature.toByteArray(), certs[i])) id = i + 1;
            }
            if (id < 0) throw new AssertionError("unrecognized certificate");
            if (!result.isEmpty()) result += ",";
            result += id;
            if (flags) result += ":" + signature.getFlags();
        }
        return result;
    }
    private static android.content.pm.SigningDetails details(int[] current, int[] past, int[] flags) {
        android.content.pm.Signature[] signatures = new android.content.pm.Signature[current.length];
        for (int i = 0; i < current.length; i++) signatures[i] = new android.content.pm.Signature(new byte[]{(byte) current[i]});
        android.content.pm.Signature[] lineage = past == null ? null : new android.content.pm.Signature[past.length];
        if (lineage != null) {
            for (int i = 0; i < past.length; i++) {
                lineage[i] = new android.content.pm.Signature(new byte[]{(byte) past[i]});
                lineage[i].setFlags(flags[i]);
            }
        }
        return new android.content.pm.SigningDetails(signatures, 3, null, lineage);
    }

}
