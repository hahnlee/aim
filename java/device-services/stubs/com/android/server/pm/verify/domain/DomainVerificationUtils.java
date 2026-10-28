// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.verify.domain;
public class DomainVerificationUtils {
    private DomainVerificationUtils() {}
    public static android.content.pm.PackageManager.NameNotFoundException throwPackageUnavailable(String name)
            throws android.content.pm.PackageManager.NameNotFoundException { throw new RuntimeException("stub"); }
    public static boolean isValidDomain(String domain) { throw new RuntimeException("stub"); }
}
