// Compile-only pinned image API; checked by the device-services build node.
package android.system;
public final class Os {
    private Os() { throw new RuntimeException("stub"); }
    public static int geteuid() { throw new RuntimeException("stub"); }
    public static void seteuid(int uid) throws ErrnoException { throw new RuntimeException("stub"); }
    public static void link(String oldPath, String newPath) throws ErrnoException { throw new RuntimeException("stub"); }
    public static void chmod(String path, int mode) throws ErrnoException { throw new RuntimeException("stub"); }
    public static void unlink(String path) throws ErrnoException { throw new RuntimeException("stub"); }
    public static long sysconf(int name){throw new RuntimeException("stub");}
    public static StructStat stat(String path)throws ErrnoException{throw new RuntimeException("stub");}
    public static StructStat fstat(java.io.FileDescriptor descriptor)throws ErrnoException{throw new RuntimeException("stub");}
 public static StructStat lstat(String path)throws ErrnoException{throw new RuntimeException("stub");}
}
