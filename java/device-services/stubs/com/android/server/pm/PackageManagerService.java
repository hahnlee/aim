// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm;
public class PackageManagerService {
    public PackageManagerService(PackageManagerServiceInjector injector, boolean factoryTest, String incremental, boolean eng, boolean userdebug, int sdk, String fingerprint) { throw new RuntimeException("stub"); }

    public static class FindPreferredActivityBodyResult { public boolean mChanged; public android.content.pm.ResolveInfo mPreferredResolveInfo; }
    public static void renameStaticSharedLibraryPackage(com.android.internal.pm.parsing.pkg.ParsedPackage pkg) {
        throw new RuntimeException("stub");
    }
    public static PackageManagerService main(android.content.Context context, Installer installer,
            com.android.server.pm.verify.domain.DomainVerificationService domains, boolean factoryTest) {
        throw new RuntimeException("stub");
    }

}
