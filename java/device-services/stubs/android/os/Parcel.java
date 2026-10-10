// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class Parcel {
    public boolean pushAllowFds(boolean allow) { throw new RuntimeException("stub"); }
    public void restoreAllowFds(boolean previous) { throw new RuntimeException("stub"); }
    public void writeParcelable(Parcelable value, int flags) { throw new RuntimeException("stub"); }
    private Parcel(long nativePtr) { throw new RuntimeException("stub"); }
    public static Parcel obtain() { throw new RuntimeException("stub"); }
    public static Parcel obtain(IBinder binder) { throw new RuntimeException("stub"); }
    public final void writeFileDescriptor(java.io.FileDescriptor fd) { throw new RuntimeException("stub"); }
    public final byte[] readBlob() { throw new RuntimeException("stub"); }
    public boolean hasFileDescriptors() { throw new RuntimeException("stub"); }
    public final void setPropagateAllowBlocking() { throw new RuntimeException("stub"); }
    public final void recycle() { throw new RuntimeException("stub"); }
    public final void setDataPosition(int pos) { throw new RuntimeException("stub"); }
    public final void writeInterfaceToken(String interfaceName) { throw new RuntimeException("stub"); }
    public final void enforceInterface(String interfaceName) { throw new RuntimeException("stub"); }
    public final void writeString(String val) { throw new RuntimeException("stub"); }
    public final String readString() { throw new RuntimeException("stub"); }
    public final void writeInt(int val) { throw new RuntimeException("stub"); }
    public final float readFloat() { throw new RuntimeException("stub"); }
    public final int readInt() { throw new RuntimeException("stub"); }
    public final void writeLong(long val) { throw new RuntimeException("stub"); }
    public final long readLong() { throw new RuntimeException("stub"); }
    public final int dataPosition() { throw new RuntimeException("stub"); }
    public final void writeString8(String val) { throw new RuntimeException("stub"); }
    public final void appendFrom(Parcel parcel, int offset, int length) { throw new RuntimeException("stub"); }
    public final String readString8() { throw new RuntimeException("stub"); }
    public final String[] createString8Array() { throw new RuntimeException("stub"); }
    public final void writeStringList(java.util.List<String> value) { throw new RuntimeException("stub"); }
    public final java.util.ArrayList<String> createStringArrayList() { throw new RuntimeException("stub"); }
    public final <T> T readParcelable(ClassLoader loader, Class<T> clazz) { throw new RuntimeException("stub"); }
    public final <T> java.util.ArrayList<T> readArrayList(ClassLoader loader, Class<? extends T> clazz) { throw new RuntimeException("stub"); }
    public final java.util.ArrayList readArrayList(ClassLoader loader) { throw new RuntimeException("stub"); }
    public final <T extends Parcelable> java.util.List<T> readParcelableList(java.util.List<T> list, ClassLoader loader, Class<T> clazz) { throw new RuntimeException("stub"); }
    public final int dataAvail() { throw new RuntimeException("stub"); }
    public final void writeByteArray(byte[] b) { throw new RuntimeException("stub"); }
    public final void writeSerializable(java.io.Serializable value) { throw new RuntimeException("stub"); }
    public final byte[] createByteArray() { throw new RuntimeException("stub"); }
    public final byte[] marshall() { throw new RuntimeException("stub"); }
    public final void unmarshall(byte[] data, int offset, int length) { throw new RuntimeException("stub"); }
    public final void writeBoolean(boolean val) { throw new RuntimeException("stub"); }
    public final boolean readBoolean() { throw new RuntimeException("stub"); }
    public final void writeException(Exception exception) { throw new RuntimeException("stub"); }
    public final void writeNoException() { throw new RuntimeException("stub"); }
    public void enforceNoDataAvail() { throw new RuntimeException("stub"); }
    public final void readException() { throw new RuntimeException("stub"); }
    public final void writeStrongInterface(IInterface val) { throw new RuntimeException("stub"); }
    public final IBinder readStrongBinder() { throw new RuntimeException("stub"); }
    public final void writeStrongBinder(IBinder val) { throw new RuntimeException("stub"); }
    public final String[] createStringArray() { throw new RuntimeException("stub"); }
    public final void writeStringArray(String[] val) { throw new RuntimeException("stub"); }
    public final int[] createIntArray() { throw new RuntimeException("stub"); }
    public final void writeIntArray(int[] val) { throw new RuntimeException("stub"); }
    public final <T extends Parcelable> void writeTypedObject(T val, int parcelableFlags) { throw new RuntimeException("stub"); }
    public final <T> java.util.ArrayList<T> createTypedArrayList(Parcelable.Creator<T> creator) { throw new RuntimeException("stub"); }
    public final <T> T readTypedObject(Parcelable.Creator<T> c) { throw new RuntimeException("stub"); }
    public long[] createLongArray() { throw new RuntimeException("stub"); }
    public void writeLongArray(long[] value) { throw new RuntimeException("stub"); }
    public final PersistableBundle readPersistableBundle() { throw new RuntimeException("stub"); }
    public final void writePersistableBundle(PersistableBundle value) { throw new RuntimeException("stub"); }
    public final <T extends Parcelable> void writeTypedArray(T[] value, int flags) { throw new RuntimeException("stub"); }
    public final <T> T[] createTypedArray(Parcelable.Creator<T> creator) { throw new RuntimeException("stub"); }

    public final void writeBooleanArray(boolean[] values) { throw new RuntimeException("stub"); }
    public final boolean[] createBooleanArray() { throw new RuntimeException("stub"); }
    public final double readDouble() { throw new RuntimeException("stub"); }
 public void writeBinderArray(IBinder[] value){throw new RuntimeException("stub");}
 public IBinder[] createBinderArray(){throw new RuntimeException("stub");}
 public void writeFloat(float value){throw new RuntimeException("stub");}
}
