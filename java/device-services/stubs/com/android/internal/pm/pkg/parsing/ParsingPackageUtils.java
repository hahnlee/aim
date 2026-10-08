// Compile-only pinned image API; checked by the device-services build node.
package com.android.internal.pm.pkg.parsing;
public class ParsingPackageUtils {
    public static boolean sCompatibilityModeEnabled;
    public ParsingPackageUtils(String[] processes, android.util.DisplayMetrics metrics,
            java.util.List<?> permissions, Callback callback) { throw new RuntimeException("stub"); }
    public interface Callback {
        boolean hasFeature(String feature);
        ParsingPackage startParsingPackage(String name, String base, String path,
                android.content.res.TypedArray attributes, boolean core);
        java.util.Set<String> getHiddenApiWhitelistedApps();
        java.util.Set<String> getInstallConstraintsAllowlist();
    }
 public static void setCompatibilityModeEnabled(boolean enabled){throw new RuntimeException("stub");}
}
