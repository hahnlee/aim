// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

// An interface here: the image's class has no constructor left for a stub
// class to declare, and a static call compiles to the same invoke-static.
public interface LocalManagerRegistry {
    static <T> T getManager(Class<T> managerClass) { throw new RuntimeException("stub"); }
}
