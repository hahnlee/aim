// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class PackageInfo implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<PackageInfo> CREATOR = null;
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public SigningInfo signingInfo;
    public String[] requestedPermissions;
    public ApplicationInfo applicationInfo;
    public ProviderInfo[] providers;
}
