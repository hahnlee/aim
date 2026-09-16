package android.util;
import java.io.*;
public final class AtomicFile {
    public AtomicFile(File file) { throw new UnsupportedOperationException("Not a persistence test"); }
    public FileOutputStream startWrite() throws IOException { throw new UnsupportedOperationException(); }
    public void finishWrite(FileOutputStream stream) { throw new UnsupportedOperationException(); }
    public void failWrite(FileOutputStream stream) { throw new UnsupportedOperationException(); }
    public FileInputStream openRead() throws FileNotFoundException { throw new UnsupportedOperationException(); }
}
