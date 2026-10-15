// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class UserHandle implements Parcelable {
 public static final int USER_NULL=-10000;
 public static final int USER_SYSTEM=0;
 public static final int USER_ALL=-1; public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(Parcel out,int flags){throw new RuntimeException("stub");}
    public static final UserHandle ALL = null;
    public static final UserHandle CURRENT = null;
    public static final int USER_CURRENT = -2;

    public int getIdentifier() { throw new RuntimeException("stub"); }
    public UserHandle(int userId) { throw new RuntimeException("stub"); }
    public static int parseUserArg(String value) { throw new RuntimeException("stub"); }
    public static UserHandle of(int userId) { throw new RuntimeException("stub"); }
    public static int getAppId(int uid) { throw new RuntimeException("stub"); }
    public static int getUserId(int uid) { throw new RuntimeException("stub"); }
    public static int getUid(int userId, int appId) { throw new RuntimeException("stub"); }
    public static boolean isCore(int uid) { throw new RuntimeException("stub"); }
}
