// Compile-only original image type; no runtime implementation.
package android.content.pm;
public class InstrumentationInfo extends PackageItemInfo implements android.os.Parcelable {
    public String targetPackage;
    public String sourceDir;
    public InstrumentationInfo() { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
}
