// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.parsing;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.PackageStateInternal;
public abstract class PackageInfoUtils {
    private PackageInfoUtils() { throw new RuntimeException("stub"); }
    public static int appInfoFlags(AndroidPackage pkg, PackageStateInternal setting) { throw new RuntimeException("stub"); }
    public static int appInfoPrivateFlags(AndroidPackage pkg, PackageStateInternal setting) { throw new RuntimeException("stub"); }
}
