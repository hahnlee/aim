// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

final class PackageAbiHelperImpl implements PackageAbiHelper {
    PackageAbiHelperImpl() {}
    public Abis getBundledAppAbis(com.android.server.pm.pkg.AndroidPackage pkg) {
        throw new RuntimeException("stub");
    }
    public String getAdjustedAbiForSharedUser(
            android.util.ArraySet<? extends com.android.server.pm.pkg.PackageStateInternal> members,
            com.android.server.pm.pkg.AndroidPackage scanned) { throw new RuntimeException("stub"); }
    public NativeLibraryPaths deriveNativeLibraryPaths(
            com.android.server.pm.pkg.AndroidPackage pkg, boolean system,
            boolean updated, java.io.File appLib32InstallDir) {
        throw new RuntimeException("stub");
    }
}
