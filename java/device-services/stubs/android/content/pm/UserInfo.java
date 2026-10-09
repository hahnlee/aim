// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class UserInfo {
    public boolean isManagedProfile() { throw new RuntimeException("stub"); }
    public int id;
    public int flags;
    public int profileGroupId;
    public boolean isCloneProfile() { throw new RuntimeException("stub"); }
    public boolean preCreated;
    public UserInfo() { throw new RuntimeException("stub"); }
    public static final int FLAG_EPHEMERAL=0x100;
    public static final int FLAG_FOR_TESTING=0x8000;
    public static String getDefaultUserType(int flags){throw new RuntimeException("stub");}
}
