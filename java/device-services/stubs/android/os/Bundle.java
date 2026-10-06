// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class Bundle extends BaseBundle {
    public Bundle() { throw new RuntimeException("stub"); }
    public <T> java.util.ArrayList<T> getParcelableArrayList(String key, Class<T> type) { throw new RuntimeException("stub"); }
    public <T extends Parcelable> void putParcelableArrayList(String key, java.util.ArrayList<T> values) { throw new RuntimeException("stub"); }
    public void putBinder(String key, IBinder value) { throw new RuntimeException("stub"); }
}
