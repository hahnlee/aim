package com.android.server.pm;

import com.android.internal.pm.parsing.PackageParser2;
import com.android.internal.pm.parsing.pkg.PackageImpl;
import java.security.PublicKey;
import android.util.ArraySet;

public final class KeySetOwnerOracle {
    private static void snapshot(String name, KeySetManagerService owner, PackageSetting[] packages) throws Exception {
        System.out.println("STATE " + name);
        for (var pkg : packages) {
            var data = pkg.getKeySetData();
            System.out.println("PACKAGE " + pkg.getPackageName() + " " + data.getProperSigningKeySet());
            if (data.getProperSigningKeySet() != -1)
                System.out.println("REF " + data.getProperSigningKeySet() + " " + owner.getSigningKeySetByPackageNameLPr(pkg.getPackageName()).getRefCountLPr());
            for (var alias : data.getAliases().entrySet()) {
                System.out.println("ALIAS " + pkg.getPackageName() + " " + alias.getKey() + " " + alias.getValue());
                System.out.println("REF " + alias.getValue() + " " + owner.getKeySetByAliasAndPackageNameLPr(pkg.getPackageName(), alias.getKey()).getRefCountLPr());
            }
            if (data.getUpgradeKeySets() != null) for (long id : data.getUpgradeKeySets())
                System.out.println("UPGRADE " + pkg.getPackageName() + " " + id);
        }
        var bytes = new java.io.ByteArrayOutputStream();
        var xml = android.util.Xml.resolveSerializer(bytes);
        xml.startDocument(null, true);
        xml.startTag(null, "packages");
        owner.writeKeySetManagerServiceLPr(xml);
        xml.endTag(null, "packages");
        xml.endDocument();
        var hex = new StringBuilder();
        for (byte b : bytes.toByteArray()) hex.append(String.format("%02x", b & 255));
        System.out.println("GLOBAL " + hex);
    }

    public static void main(String[] args) throws Exception {
        try (var parser = new PackageParser2(null, null, null, new PackageParser2.Callback() {
            public boolean hasFeature(String feature) { return false; }
            public java.util.Set<String> getHiddenApiWhitelistedApps() { return java.util.Set.of(); }
            public java.util.Set<String> getInstallConstraintsAllowlist() { return java.util.Set.of(); }
            public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) { return false; }
        })) {
            ArraySet<PublicKey> one = ((PackageImpl)parser.parsePackage(new java.io.File(args[0]), 0, false)).getKeySetMapping().get("z");
            ArraySet<PublicKey> two = ((PackageImpl)parser.parsePackage(new java.io.File(args[1]), 0, false)).getKeySetMapping().get("a");
            var packages = new PackageSetting[] {
                new PackageSetting("a", null, new java.io.File("/data/app/a"), 0, 0, java.util.UUID.randomUUID()),
                new PackageSetting("b", null, new java.io.File("/data/app/b"), 0, 0, java.util.UUID.randomUUID())
            };
            var map = new com.android.server.utils.WatchedArrayMap<String, PackageSetting>();
            for (var pkg : packages) map.put(pkg.getPackageName(), pkg);
            var owner = new KeySetManagerService(map);
            owner.addSigningKeySetToPackageLPw(packages[0], one);
            owner.addDefinedKeySetsToPackageLPw(packages[0], java.util.Map.of("next", two));
            owner.addUpgradeKeySetsToPackageLPw(packages[0], java.util.Set.of("next"));
            snapshot("first", owner, packages);
            owner.addSigningKeySetToPackageLPw(packages[1], one);
            owner.addDefinedKeySetsToPackageLPw(packages[1], java.util.Map.of());
            snapshot("shared", owner, packages);
            owner.addSigningKeySetToPackageLPw(packages[0], two);
            owner.addDefinedKeySetsToPackageLPw(packages[0], java.util.Map.of());
            snapshot("rotate", owner, packages);
            owner.removeAppKeySetDataLPw("b");
            snapshot("remove-shared", owner, packages);
            owner.removeAppKeySetDataLPw("a");
            snapshot("remove-last", owner, packages);
            owner.addSigningKeySetToPackageLPw(packages[0], one);
            snapshot("reallocate", owner, packages);
            // Read a saved pool containing a live set, an orphan set sharing
            // its key, an orphan-only key and a wholly unused public key.
            long live = packages[0].getKeySetData().getProperSigningKeySet();
            String first = java.util.Base64.getEncoder().encodeToString(one.iterator().next().getEncoded());
            String second = java.util.Base64.getEncoder().encodeToString(two.iterator().next().getEncoded());
            String document = "<keyset-settings version='1'><keys>"
                + "<public-key identifier='3' value='" + first + "'/>"
                + "<public-key identifier='4' value='" + second + "'/>"
                + "<public-key identifier='5' value='" + first + "'/>"
                + "</keys><keysets><keyset identifier='" + live + "'><key-id identifier='3'/></keyset>"
                + "<keyset identifier='4'><key-id identifier='3'/><key-id identifier='4'/></keyset>"
                + "</keysets><lastIssuedKeyId value='20'/><lastIssuedKeySetId value='30'/></keyset-settings>";
            var input = android.util.Xml.resolvePullParser(new java.io.ByteArrayInputStream(document.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
            while (!"keyset-settings".equals(input.getName())) input.next();
            // Reuse an empty original ArrayMap, without constructing or
            // modifying an actual package's alias map.
            var holder = new PackageSetting("refs", null, new java.io.File("/data/app/refs"), 0, 0, java.util.UUID.randomUUID());
            @SuppressWarnings({"unchecked", "rawtypes"})
            android.util.ArrayMap<Long, Integer> refs = (android.util.ArrayMap)(holder.getKeySetData().getAliases());
            refs.put(live, 1);
            owner = new KeySetManagerService(map);
            owner.readKeySetsLPw(input, refs);
            snapshot("restore-orphan", owner, packages);
            // Settings increments every persisted role before replacing aliases.
            refs.clear();
            for (var pkg : packages) pkg.getKeySetData().removeAllDefinedKeySets();
            packages[0].getKeySetData().setProperSigningKeySet(-1);
            try (var stream = new java.io.FileInputStream("/data/local/tmp/manifest-keysets/refs.xml")) {
                input = android.util.Xml.resolvePullParser(stream);
                owner = new KeySetManagerService(map);
                int event;
                while ((event = input.next()) != 1) {
                    if (event != 2) continue;
                    String tag = input.getName();
                    if (tag.equals("proper-signing-keyset") || tag.equals("defined-keyset")) {
                        long id = input.getAttributeLong(null, "identifier");
                        Integer count = refs.get(id);
                        refs.put(id, count == null ? 1 : count + 1);
                        if (tag.equals("proper-signing-keyset")) packages[0].getKeySetData().setProperSigningKeySet(id);
                        else packages[0].getKeySetData().addDefinedKeySet(id, input.getAttributeValue(null, "alias"));
                    } else if (tag.equals("keyset-settings")) owner.readKeySetsLPw(input, refs);
                }
            }
            snapshot("import-replaced", owner, packages);
            owner.addSigningKeySetToPackageLPw(packages[0], one);
            owner.addDefinedKeySetsToPackageLPw(packages[0], java.util.Map.of("new", two));
            snapshot("replace-imported", owner, packages);
            var signingHandle = owner.getSigningKeySetByPackageNameLPr("a");
            var aliasHandle = owner.getKeySetByAliasAndPackageNameLPr("a", "new");
            owner.removeAppKeySetDataLPw("a");
            if (signingHandle.getRefCountLPr() != 2 || aliasHandle.getRefCountLPr() != 1) throw new AssertionError("imported residual references differ");
            snapshot("remove-imported", owner, packages);
            refs.clear();
            input = android.util.Xml.resolvePullParser(new java.io.ByteArrayInputStream(document.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
            while (!"keyset-settings".equals(input.getName())) input.next();
            owner = new KeySetManagerService(map);
            owner.readKeySetsLPw(input, refs);
            snapshot("restart-imported", owner, packages);
        }
    }
}
