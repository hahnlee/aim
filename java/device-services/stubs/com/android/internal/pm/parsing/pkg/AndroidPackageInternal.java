// Compile-only pinned image API; checked by the device-services build node.
package com.android.internal.pm.parsing.pkg;
public interface AndroidPackageInternal extends com.android.server.pm.pkg.AndroidPackage {
    String[] getUsesLibrariesSorted();
    String[] getUsesOptionalLibrariesSorted();
    String[] getUsesSdkLibrariesSorted();
    String[] getUsesStaticLibrariesSorted();
}
