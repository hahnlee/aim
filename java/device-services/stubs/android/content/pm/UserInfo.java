// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class UserInfo implements android.os.Parcelable {
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public static final android.os.Parcelable.Creator<UserInfo> CREATOR = null;
    public boolean supportsSwitchToByUser() { throw new RuntimeException("stub"); }
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
