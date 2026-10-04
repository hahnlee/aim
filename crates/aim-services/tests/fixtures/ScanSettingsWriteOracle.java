package com.android.server.pm;

/** Actual pinned Settings writer on detached, native-captured setting owners. */
public final class ScanSettingsWriteOracle {
    public static void write(java.io.File cache, dev.aim.server.PackageScanLease lease,
            PackageSetting assembled) throws Exception {
        verifyArrayMapOrder();
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
    private static void verifyArrayMapOrder() {
        var map = new android.util.ArrayMap<String, Integer>();
        map.put("BB", 1); map.put("Aa", 2);
        if (!new java.util.ArrayList<>(map.keySet()).get(0).equals("BB") || !new java.util.ArrayList<>(map.keySet()).get(1).equals("Aa"))
            throw new AssertionError("original ArrayMap collision insertion order");
        map.put("BB", 3);
        if (!new java.util.ArrayList<>(map.keySet()).get(0).equals("BB")) throw new AssertionError("original ArrayMap lookup order");
        map.remove("BB"); map.put("BB", 4);
        if (!new java.util.ArrayList<>(map.keySet()).get(0).equals("Aa") || !new java.util.ArrayList<>(map.keySet()).get(1).equals("BB"))
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
