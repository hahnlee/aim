// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.adservices.shared.system;

import android.content.Context;

public final class SystemContextSingleton {
    private SystemContextSingleton() { throw new RuntimeException("stub"); }

    public static Context set(Context context) { throw new RuntimeException("stub"); }
}
