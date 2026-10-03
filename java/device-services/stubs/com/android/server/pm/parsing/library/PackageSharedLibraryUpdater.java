// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.parsing.library;
public abstract class PackageSharedLibraryUpdater {
    public abstract void updatePackage(com.android.internal.pm.parsing.pkg.ParsedPackage pkg,
            boolean system, boolean updated);
}
