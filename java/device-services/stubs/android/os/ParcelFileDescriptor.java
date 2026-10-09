// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

import java.io.FileDescriptor;

public class ParcelFileDescriptor implements Parcelable, java.io.Closeable {
    public static final Parcelable.Creator<ParcelFileDescriptor> CREATOR = null;
    public static final int MODE_READ_ONLY = 0x10000000;
    public static ParcelFileDescriptor open(java.io.File file, int mode) throws java.io.FileNotFoundException { throw new RuntimeException("stub"); }

    public static class AutoCloseInputStream extends java.io.FileInputStream {
        public AutoCloseInputStream(ParcelFileDescriptor descriptor) { super(descriptor.getFileDescriptor()); }
    }

    public static ParcelFileDescriptor fromFd(int fd) throws java.io.IOException { throw new RuntimeException("stub"); }
    public int getFd(){throw new RuntimeException("stub");}
    public int detachFd(){throw new RuntimeException("stub");}
    public static ParcelFileDescriptor adoptFd(int fd){throw new RuntimeException("stub");}
    public static class AutoCloseOutputStream extends java.io.FileOutputStream {
        public AutoCloseOutputStream(ParcelFileDescriptor descriptor){super(descriptor.getFileDescriptor());}
        @Override public void close() throws java.io.IOException {throw new RuntimeException("stub");}
    }
    public FileDescriptor getFileDescriptor() { throw new RuntimeException("stub"); }
    @Override public void close() throws java.io.IOException { throw new RuntimeException("stub"); }
    public ParcelFileDescriptor(FileDescriptor fd) { throw new RuntimeException("stub"); }
    @Override
    public void writeToParcel(Parcel out, int flags) { throw new RuntimeException("stub"); }
    @Override
    public int describeContents() { throw new RuntimeException("stub"); }
}
