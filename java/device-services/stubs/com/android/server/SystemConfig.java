// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

import android.util.ArrayMap;
import android.util.ArraySet;

public class SystemConfig {
    public SystemConfig(boolean readPermissions) { throw new RuntimeException("stub"); }
    public static SystemConfig getInstance() { throw new RuntimeException("stub"); }
    public ArraySet<String> getAllowUnthrottledLocation() { throw new RuntimeException("stub"); }
    public ArrayMap<String, ArraySet<String>> getAllowAdasLocationSettings() { throw new RuntimeException("stub"); }
    public ArrayMap<String, ArraySet<String>> getAllowIgnoreLocationSettings() { throw new RuntimeException("stub"); }
}
