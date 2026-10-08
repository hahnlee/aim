// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public class Process {
    public static int myPid() { throw new RuntimeException("stub"); }
    public static int myUid() { throw new RuntimeException("stub"); }
    public static final int ROOT_UID = 0;
    public static final int SHELL_UID = 2000;
    public static final int SYSTEM_UID = 1000;
 public static boolean isIsolatedUid(int uid){throw new RuntimeException("stub");}
}
