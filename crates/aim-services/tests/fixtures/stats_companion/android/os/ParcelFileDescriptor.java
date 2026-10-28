// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
import java.io.*;
public class ParcelFileDescriptor implements AutoCloseable {
    public static int open;
    private final FileOutputStream file;
    private final java.nio.file.Path path;
    private boolean closed;
    ParcelFileDescriptor() throws IOException {
        path=java.nio.file.Files.createTempFile("stats-uid-map", ".pipe");
        file=new FileOutputStream(path.toFile()); open++;
    }
    public static ParcelFileDescriptor[] createPipe() throws IOException {
        return new ParcelFileDescriptor[] {new ParcelFileDescriptor(),new ParcelFileDescriptor()};
    }
    public java.io.FileDescriptor getFileDescriptor() { try { return file.getFD(); } catch(IOException e) { throw new AssertionError(e); } }
    public void close() throws IOException {
        if(!closed) { closed=true; open--; file.close(); java.nio.file.Files.delete(path); }
    }
    public static class AutoCloseOutputStream extends FileOutputStream {
        private final ParcelFileDescriptor owner;
        public AutoCloseOutputStream(ParcelFileDescriptor fd) {
            super(fd.getFileDescriptor()); owner=fd;
        }
        @Override public void close() throws IOException { super.close(); owner.close(); }
    }
}
