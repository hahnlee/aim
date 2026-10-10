// Compile-only pinned image API; checked by the device-services build node.
package com.android.internal.pm.parsing;
public class PackageParser2 implements AutoCloseable {
    public PackageParser2(String[] processes, android.util.DisplayMetrics metrics,
            com.android.internal.pm.parsing.IPackageCacher cacher, Callback callback) {
        throw new RuntimeException("stub");
    }
    public com.android.internal.pm.parsing.pkg.ParsedPackage parsePackage(
            java.io.File file, int flags, boolean useCaches)
            throws com.android.internal.pm.parsing.PackageParserException {
        throw new RuntimeException("stub");
    }
    public void close() { throw new RuntimeException("stub"); }
    public abstract static class Callback implements com.android.internal.pm.pkg.parsing.ParsingPackageUtils.Callback {
        public final com.android.internal.pm.pkg.parsing.ParsingPackage startParsingPackage(
                String name, String base, String path, android.content.res.TypedArray attributes,
                boolean core) { throw new RuntimeException("stub"); }
        public abstract boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info);
    }
}
