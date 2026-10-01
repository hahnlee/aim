// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

import android.content.pm.SharedLibraryInfo;
import android.content.pm.VersionedPackage;

import java.util.List;

public interface SharedLibrary {
    String getName();
    String getPath();
    String getPackageName();
    List<String> getAllCodePaths();
    long getVersion();
    int getType();
    boolean isNative();
    VersionedPackage getDeclaringPackage();
    List<VersionedPackage> getDependentPackages();
    List<SharedLibraryInfo> getDependencies();
}
