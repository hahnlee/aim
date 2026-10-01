// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class RemoteCallback {
    public RemoteCallback(OnResultListener listener) { throw new RuntimeException("stub"); }

    public interface OnResultListener {
        void onResult(Bundle result);
    }
}
