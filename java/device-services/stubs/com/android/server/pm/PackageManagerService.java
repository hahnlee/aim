// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm;
public interface PackageManagerService {
    static void renameStaticSharedLibraryPackage(com.android.internal.pm.parsing.pkg.ParsedPackage pkg) {
        throw new RuntimeException("stub");
    }
    static PackageManagerService main(android.content.Context context, Installer installer,
            com.android.server.pm.verify.domain.DomainVerificationService domains, boolean factoryTest) {
        throw new RuntimeException("stub");
    }
}
