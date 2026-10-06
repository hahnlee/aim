// Compile-only pinned image API; checked by the device-services build node.
package android.system;
public final class Os {
    private Os() { throw new RuntimeException("stub"); }
    public static int geteuid() { throw new RuntimeException("stub"); }
    public static void seteuid(int uid) throws ErrnoException { throw new RuntimeException("stub"); }
}
