// Compile-only original image type; no runtime implementation.
package android.content.pm;
public class ProcessInfo implements android.os.Parcelable {
    public String name;
    public static final android.os.Parcelable.Creator<ProcessInfo> CREATOR = null;
    public ProcessInfo(ProcessInfo other) { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
}
