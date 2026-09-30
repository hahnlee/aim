// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

import android.os.Handler;

public final class FgThread extends ServiceThread {
    private FgThread() { super(null, 0, false); throw new RuntimeException("stub"); }
    public static Handler getHandler() { throw new RuntimeException("stub"); }
}
