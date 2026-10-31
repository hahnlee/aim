// Compilation ABI, AOSP android-16.0.0_r1 (Apache-2.0).
package android.view;
public class WindowInfo {
    public int type, layer, displayId;
    public android.os.IBinder token;
    public boolean focused;
    public CharSequence title;
    public static WindowInfo obtain() { throw new RuntimeException("stub"); }
}
