package android.os;
public final class ParcelFileDescriptor implements Parcelable, java.io.Closeable {
    public static final int MODE_READ_ONLY = 0x10000000;
    public static final Parcelable.Creator<ParcelFileDescriptor> CREATOR = null;
    private final java.io.FileInputStream stream;
    private ParcelFileDescriptor(java.io.FileInputStream stream) { this.stream = stream; }
    public static ParcelFileDescriptor open(java.io.File file, int mode) throws java.io.FileNotFoundException {
        if (mode != MODE_READ_ONLY) throw new AssertionError(mode);
        return new ParcelFileDescriptor(new java.io.FileInputStream(file));
    }
    public java.io.FileDescriptor getFileDescriptor() {
        try { return stream.getFD(); } catch (java.io.IOException failure) { throw new AssertionError(failure); }
    }
    public static final class AutoCloseInputStream extends java.io.FileInputStream {
        public AutoCloseInputStream(ParcelFileDescriptor descriptor) { super(descriptor.getFileDescriptor()); }
    }
    public void close() throws java.io.IOException { stream.close(); }
    public void writeToParcel(Parcel out, int flags) { throw new AssertionError(); }
    public int describeContents() { return 0; }
}
