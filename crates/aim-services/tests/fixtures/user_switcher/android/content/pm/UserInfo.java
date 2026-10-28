package android.content.pm;
public class UserInfo implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<UserInfo> CREATOR = null;
    private final boolean switchable;
    public UserInfo(boolean switchable) { this.switchable = switchable; }
    public boolean supportsSwitchToByUser() { return switchable; }
    public void writeToParcel(android.os.Parcel p, int flags) { throw new AssertionError(); }
    public int describeContents() { return 0; }
}
