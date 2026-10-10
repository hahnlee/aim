// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public final class ProviderInfo extends ComponentInfo implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<ProviderInfo> CREATOR = null;
    public String authority;
    public boolean isSyncable;

 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel dest,int flags){throw new RuntimeException("stub");}
}
