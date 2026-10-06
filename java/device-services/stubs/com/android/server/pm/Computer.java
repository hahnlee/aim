// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public interface Computer {
    com.android.server.pm.pkg.PackageStateInternal getPackageStateInternal(String name);
}
