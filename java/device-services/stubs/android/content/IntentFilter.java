// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

public class IntentFilter implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<IntentFilter> CREATOR = null;
    public IntentFilter(IntentFilter original) { throw new RuntimeException("stub"); }
    public IntentFilter() { throw new RuntimeException("stub"); }
    public IntentFilter(String action) { throw new RuntimeException("stub"); }
    public static class MalformedMimeTypeException extends android.util.AndroidException {
        public MalformedMimeTypeException(String type) { throw new RuntimeException("stub"); }
    }
    public final void addDynamicDataType(String type) throws MalformedMimeTypeException { throw new RuntimeException("stub"); }
    public final void addAction(String action) { throw new RuntimeException("stub"); }
    public final void addDataScheme(String scheme) { throw new RuntimeException("stub"); }

 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel dest,int flags){throw new RuntimeException("stub");}
}
