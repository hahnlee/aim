package android.os;
public class ParcelFileDescriptor implements java.io.Closeable {
 public void close() throws java.io.IOException {} private final java.io.FileDescriptor fd;
 public ParcelFileDescriptor(java.io.FileDescriptor f){fd=f;} public java.io.FileDescriptor getFileDescriptor(){return fd;}
}
