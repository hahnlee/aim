// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public abstract class FileObserver {
    public static final int CLOSE_WRITE = 0x00000008;
    public static final int MOVED_TO = 0x00000080;

    public FileObserver(java.io.File file, int mask) { throw new RuntimeException("stub"); }
    public abstract void onEvent(int event, String path);
    public void startWatching() { throw new RuntimeException("stub"); }
}
