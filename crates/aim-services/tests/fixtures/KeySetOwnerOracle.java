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
            for (var alias : data.getAliases().entrySet())
                System.out.println("ALIAS " + pkg.getPackageName() + " " + alias.getKey() + " " + alias.getValue());
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
        }
    }
}
