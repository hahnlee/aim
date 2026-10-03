// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class UserHandle {
    public static final UserHandle ALL = null;
    public static final UserHandle CURRENT = null;
    public static final int USER_CURRENT = -2;

    public int getIdentifier() { throw new RuntimeException("stub"); }
    public UserHandle(int userId) { throw new RuntimeException("stub"); }
    public static UserHandle of(int userId) { throw new RuntimeException("stub"); }
    public static int getUserId(int uid) { throw new RuntimeException("stub"); }
}
