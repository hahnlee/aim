package com.android.server.pm;

import com.android.internal.pm.parsing.PackageParser2;
import com.android.internal.pm.parsing.pkg.PackageImpl;

public final class UpdateOwnershipResourceOracle {
    public static void main(String[] args) throws Exception {
        try (var parser = new PackageParser2(null, null, null, new PackageParser2.Callback() {
            public boolean hasFeature(String feature) { return false; }
            public java.util.Set<String> getHiddenApiWhitelistedApps() { return java.util.Set.of(); }
            public java.util.Set<String> getInstallConstraintsAllowlist() { return java.util.Set.of(); }
            public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) { return false; }
        })) {
            var owner = new UpdateOwnershipHelper();
            for (String path : args) {
                var file = new java.io.File(path);
                var pkg = (PackageImpl) parser.parsePackage(file, 0, false);
                var setting = new PackageSetting(pkg.getPackageName(), null, file, 1, 0,
                        java.util.UUID.randomUUID()).setPkg(pkg);
                var contents = owner.readUpdateOwnerDenyList(setting);
                if (contents == null) throw new IllegalStateException("original resource read failed: " + path);
                System.out.print(file.getName());
                for (String name : contents) {
                    System.out.print(" ");
                    for (byte b : name.getBytes(java.nio.charset.StandardCharsets.UTF_8))
                        System.out.printf("%02x", b & 255);
                }
                System.out.println();
            }
        }
    }
}
