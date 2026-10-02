// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.parsing.pkg;
public interface AndroidPackageUtils {
    static com.android.internal.content.NativeLibraryHelper.Handle createNativeLibraryHandle(
            com.android.server.pm.pkg.AndroidPackage pkg) throws java.io.IOException {
        throw new RuntimeException("stub");
    }
    static String getRawPrimaryCpuAbi(com.android.server.pm.pkg.AndroidPackage pkg) {
        throw new RuntimeException("stub");
    }
    static android.content.pm.ApplicationInfo generateAppInfoWithoutState(
            com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
}
