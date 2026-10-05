// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

import android.util.ArrayMap;
import android.util.ArraySet;

public class SystemConfig {
    public SystemConfig(boolean readPermissions) { throw new RuntimeException("stub"); }
    public ArraySet<String> getLinkedApps() { throw new RuntimeException("stub"); }
    public static SystemConfig getInstance() { throw new RuntimeException("stub"); }
    public String getSystemAppUpdateOwnerPackageName(String packageName) { throw new RuntimeException("stub"); }
    public ArrayMap<String, Integer> getOemDefinedUids() { throw new RuntimeException("stub"); }
    public java.util.Set<String> getInitialNonStoppedSystemPackages() { throw new RuntimeException("stub"); }
    public java.util.Set<String> getPreinstallPackagesWithStrictSignatureCheck() { throw new RuntimeException("stub"); }
    public void readPermissions(org.xmlpull.v1.XmlPullParser parser, java.io.File directory, int flags) {
        throw new RuntimeException("stub");
    }
    public ArraySet<String> getAllowUnthrottledLocation() { throw new RuntimeException("stub"); }
    public ArrayMap<String, ArraySet<String>> getAllowAdasLocationSettings() { throw new RuntimeException("stub"); }
    public ArrayMap<String, ArraySet<String>> getAllowIgnoreLocationSettings() { throw new RuntimeException("stub"); }
}
