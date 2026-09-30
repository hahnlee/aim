// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

public abstract class BroadcastReceiver {
    public BroadcastReceiver() { throw new RuntimeException("stub"); }
    public abstract void onReceive(Context context, Intent intent);
    public final void setResultCode(int code) { throw new RuntimeException("stub"); }
    public int getSendingUserId() { throw new RuntimeException("stub"); }
}
