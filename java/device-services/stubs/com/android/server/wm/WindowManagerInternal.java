// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.wm;

public abstract class WindowManagerInternal {
    public WindowManagerInternal() { throw new RuntimeException("stub"); }
    public abstract boolean isKeyguardShowingAndNotOccluded();
    public abstract void clearSnapshotCache();
}
