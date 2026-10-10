// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

public final class ComponentName implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<ComponentName> CREATOR = null;
    public ComponentName(String pkg, String cls) { throw new RuntimeException("stub"); }
    public static ComponentName unflattenFromString(String name) { throw new RuntimeException("stub"); }
    public String getPackageName() { throw new RuntimeException("stub"); }
    public String getClassName() { throw new RuntimeException("stub"); }
    public String flattenToShortString() { throw new RuntimeException("stub"); }
    public String flattenToString() { throw new RuntimeException("stub"); }

 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel dest,int flags){throw new RuntimeException("stub");}
}
