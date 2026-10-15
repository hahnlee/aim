// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.parsing.library;
public abstract class PackageBackwardCompatibility extends PackageSharedLibraryUpdater {
    private PackageBackwardCompatibility(boolean onBcp, PackageSharedLibraryUpdater[] updaters) {
        throw new RuntimeException("stub");
    }
    public static boolean bootClassPathContainsATB() { throw new RuntimeException("stub"); }
    public static void modifySharedLibraries(com.android.internal.pm.parsing.pkg.ParsedPackage pkg,
            boolean system, boolean updated) { throw new RuntimeException("stub"); }
}
