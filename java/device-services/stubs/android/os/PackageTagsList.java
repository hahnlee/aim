// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

import android.util.ArrayMap;
import android.util.ArraySet;

public final class PackageTagsList {
    private PackageTagsList(ArrayMap<String, ArraySet<String>> packageTags) { throw new RuntimeException("stub"); }
    public boolean isEmpty() { throw new RuntimeException("stub"); }

    public static final class Builder {
        public Builder() { throw new RuntimeException("stub"); }
        public Builder add(String packageName, String attributionTag) { throw new RuntimeException("stub"); }
        public PackageTagsList build() { throw new RuntimeException("stub"); }
    }
}
