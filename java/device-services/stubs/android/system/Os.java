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
    public static void socketpair(int domain,int type,int protocol,java.io.FileDescriptor a,java.io.FileDescriptor b) throws ErrnoException {throw new RuntimeException("stub");}
    public static int sendmsg(java.io.FileDescriptor fd,StructMsghdr msg,int flags) throws ErrnoException,java.net.SocketException {throw new RuntimeException("stub");}
    public static int recvmsg(java.io.FileDescriptor fd,StructMsghdr msg,int flags) throws ErrnoException,java.net.SocketException {throw new RuntimeException("stub");}
    public static int fcntlInt(java.io.FileDescriptor fd,int cmd,int arg) throws ErrnoException {throw new RuntimeException("stub");}
    public static void execv(String path,String[] args) throws ErrnoException {throw new RuntimeException("stub");}
    public static java.io.FileDescriptor dup(java.io.FileDescriptor fd) throws ErrnoException {throw new RuntimeException("stub");}
    public static void close(java.io.FileDescriptor fd) throws ErrnoException {throw new RuntimeException("stub");}
    public static void fsync(java.io.FileDescriptor fd) throws ErrnoException {throw new RuntimeException("stub");}
    public static long lseek(java.io.FileDescriptor fd,long offset,int whence) throws ErrnoException {throw new RuntimeException("stub");}
    public static int write(java.io.FileDescriptor fd,byte[] data,int offset,int length) throws ErrnoException {throw new RuntimeException("stub");}
    public static int pread(java.io.FileDescriptor fd,byte[] data,int offset,int length,long position) throws ErrnoException {throw new RuntimeException("stub");}
}
