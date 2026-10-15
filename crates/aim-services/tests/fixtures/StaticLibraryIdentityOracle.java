package com.android.server.pm;

public final class StaticLibraryIdentityOracle {
    public static void verify(java.io.File directory) throws Exception {
        byte[] input = java.nio.file.Files.readAllBytes(new java.io.File(directory, "static-identity.input").toPath());
        for (boolean active : new boolean[] {false, true}) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(input);
            // addForInitLI's pinned active-APEX guard, with the original rename owner.
            if (!active) PackageManagerService.renameStaticSharedLibraryPackage(pkg);
            java.nio.file.Files.write(new java.io.File(directory, "static-identity-" + active + ".original").toPath(),
                com.android.server.pm.parsing.PackageCacher.toCacheEntryStatic(pkg));
        }
    }
    private StaticLibraryIdentityOracle() {}
}
