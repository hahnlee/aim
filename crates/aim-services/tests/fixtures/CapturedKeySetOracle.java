package com.android.server.pm;

/** Pinned Settings keyset branch port; mutations run on original PackageKeySetData. */
public final class CapturedKeySetOracle {
    public static void verifyReplica(com.android.server.pm.pkg.PackageStateInternal replica,
            com.android.server.pm.pkg.PackageStateInternal original) {
        var expected = original.getKeySetData();
        var keys = replica.getKeySetData();
        if (keys.getProperSigningKeySet() != expected.getProperSigningKeySet()
                || !java.util.Arrays.equals(keys.getUpgradeKeySets(), expected.getUpgradeKeySets())
                || !keys.getAliases().equals(expected.getAliases())) throw new AssertionError("replica keysets differ");
        keys.removeAllUpgradeKeySets(); keys.removeAllDefinedKeySets();
        var fresh = replica.getKeySetData();
        if (!java.util.Arrays.equals(fresh.getUpgradeKeySets(), expected.getUpgradeKeySets())
                || !fresh.getAliases().equals(expected.getAliases())) throw new AssertionError("mutable replica keysets escaped");
    }
    public static void verifyCaptured(dev.aim.server.PackageSettingData data, boolean populated) {
        var keys = data.getKeySetData();
        if (!populated) {
            if (keys.getUpgradeKeySets() != null || !keys.getAliases().isEmpty()) throw new AssertionError("default captured keysets differ");
            return;
        }
        long id = data.keySets.properSigningKeySet;
        if (id <= 0 || keys.getProperSigningKeySet() != id || !java.util.Arrays.equals(keys.getUpgradeKeySets(), new long[] {id})
                || keys.getAliases().size() != 4 || keys.getAliases().get(null) != id || keys.getAliases().get("") != id
                || keys.getAliases().get("BB") != id || keys.getAliases().get("Aa") != id) throw new AssertionError("captured keyset getters differ");
        keys.getUpgradeKeySets()[0] = 99; keys.getAliases().clear();
        if (!java.util.Arrays.equals(data.getKeySetData().getUpgradeKeySets(), new long[] {id}) || data.getKeySetData().getAliases().size() != 4) throw new AssertionError("mutable keyset escaped capture");
        try { data.keySets.aliases.clear(); throw new AssertionError("mutable alias inputs"); } catch (UnsupportedOperationException expected) {}
    }
    public static void verify(java.io.File file) throws Exception {
        try (var out = new java.io.DataOutputStream(new java.io.FileOutputStream(file.getPath() + ".keysets-original"))) {
            for (int i = 0; i < 12; i++) {
                var data = new PackageKeySetData();
                try (var input = new java.io.FileInputStream(file.getPath() + ".keysets-" + i + ".xml")) {
                    var parser = android.util.Xml.resolvePullParser(input);
                    while (parser.next() != 2) {}
                    int depth = parser.getDepth(), event;
                    while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
                        if (event != 2) continue;
                        switch (parser.getName()) {
                            case "proper-signing-keyset" -> data.setProperSigningKeySet(parser.getAttributeLong(null, "identifier"));
                            case "upgrade-keyset" -> data.addUpgradeKeySetById(parser.getAttributeLong(null, "identifier"));
                            case "defined-keyset" -> data.addDefinedKeySet(parser.getAttributeLong(null, "identifier"), parser.getAttributeValue(null, "alias"));
                            default -> throw new AssertionError("unexpected keyset fixture tag");
                        }
                    }
                }
                out.writeLong(data.getProperSigningKeySet());
                long[] upgrades = data.getUpgradeKeySets(); out.writeInt(upgrades == null ? -1 : upgrades.length);
                if (upgrades != null) for (long id : upgrades) out.writeLong(id);
                out.writeInt(data.getAliases().size());
                for (var alias : data.getAliases().entrySet()) {
                    String name = alias.getKey(); byte[] bytes = name == null ? null : name.getBytes(java.nio.charset.StandardCharsets.UTF_8);
                    out.writeInt(bytes == null ? -1 : bytes.length); if (bytes != null) out.write(bytes);
                    out.writeLong(alias.getValue());
                }
                var copy = new PackageKeySetData(data);
                data.removeAllUpgradeKeySets(); data.removeAllDefinedKeySets();
                if (data.getUpgradeKeySets() != null || !data.getAliases().isEmpty()) throw new AssertionError("original cleared keysets differ");
                if (copy.getProperSigningKeySet() != data.getProperSigningKeySet()) throw new AssertionError("copy lost proper set");
                if (upgrades != null && !java.util.Arrays.equals(upgrades, copy.getUpgradeKeySets())) throw new AssertionError("copy lost upgrades");
            }
        }
        for (boolean binary : new boolean[] {false, true}) {
            var output = new java.io.ByteArrayOutputStream();
            var serializer = binary ? android.util.Xml.resolveSerializer(output) : android.util.Xml.newSerializer();
            if (!binary) serializer.setOutput(output, "UTF-8");
            serializer.startTag(null, "defined-keyset");
            try { serializer.attribute(null, "alias", null); throw new AssertionError("original writer accepted null alias"); }
            catch (NullPointerException expected) {}
        }
    }
}
