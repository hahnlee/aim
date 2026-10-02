// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;

interface PackageAbiHelper {
    final class Abis {
        public final String primary;
        public final String secondary;
        Abis(String primary, String secondary) { throw new RuntimeException("stub"); }
    }
    final class NativeLibraryPaths {
        public final String nativeLibraryRootDir;
        public final boolean nativeLibraryRootRequiresIsa;
        public final String nativeLibraryDir;
        public final String secondaryNativeLibraryDir;
        NativeLibraryPaths(String root, boolean requiresIsa, String primary, String secondary) {
            throw new RuntimeException("stub");
        }
    }
}
