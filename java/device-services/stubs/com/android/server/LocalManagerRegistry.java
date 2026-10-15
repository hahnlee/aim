// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

public final class LocalManagerRegistry {
    private LocalManagerRegistry() { throw new RuntimeException("stub"); }
    public static <T> void addManager(Class<T> managerClass, T manager) { throw new RuntimeException("stub"); }
    public static <T> T getManager(Class<T> managerClass) { throw new RuntimeException("stub"); }
}
