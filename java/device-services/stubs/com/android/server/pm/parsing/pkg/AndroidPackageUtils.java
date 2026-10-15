// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.parsing.pkg;
public abstract class AndroidPackageUtils {
    private AndroidPackageUtils() { throw new RuntimeException("stub"); }
    public static String getRawSecondaryCpuAbi(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
    public static com.android.internal.content.NativeLibraryHelper.Handle createNativeLibraryHandle(
            com.android.server.pm.pkg.AndroidPackage pkg) throws java.io.IOException {
        throw new RuntimeException("stub");
    }
    public static String getRawPrimaryCpuAbi(com.android.server.pm.pkg.AndroidPackage pkg) {
        throw new RuntimeException("stub");
    }
    public static android.content.pm.ApplicationInfo generateAppInfoWithoutState(
            com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
}
