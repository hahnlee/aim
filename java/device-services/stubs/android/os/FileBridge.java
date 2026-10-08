// Compile-only members checked against the pinned original image.
package android.os;
public class FileBridge extends Thread {
    public FileBridge() { throw new RuntimeException("stub"); }
    public static class FileBridgeOutputStream extends java.io.OutputStream {
        public FileBridgeOutputStream(ParcelFileDescriptor descriptor) { throw new RuntimeException("stub"); }
        public void write(int value) throws java.io.IOException { throw new RuntimeException("stub"); }
        public void fsync() throws java.io.IOException { throw new RuntimeException("stub"); }
        public void close() throws java.io.IOException { throw new RuntimeException("stub"); }
    }
}
