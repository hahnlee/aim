// Image API declarations only; checked by the device-services build node.
package com.android.server.pm;

final class ResilientAtomicFile implements java.io.Closeable {
    ResilientAtomicFile(java.io.File file, java.io.File backup, java.io.File reserve,
            int mode, String name, ReadEventLogger logger) { throw new RuntimeException("stub"); }
    public java.io.FileOutputStream startWrite() throws java.io.IOException { throw new RuntimeException("stub"); }
    public void finishWrite(java.io.FileOutputStream stream, boolean verity) throws java.io.IOException { throw new RuntimeException("stub"); }
    public java.io.FileInputStream openRead() throws java.io.IOException { throw new RuntimeException("stub"); }
    public void failRead(java.io.FileInputStream stream, Exception error) { throw new RuntimeException("stub"); }
    public void failWrite(java.io.FileOutputStream stream) { throw new RuntimeException("stub"); }
    public void close() { throw new RuntimeException("stub"); }
    interface ReadEventLogger { void logEvent(int priority, String message); }
}
