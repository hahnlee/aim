// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class Parcel {
    private Parcel(long nativePtr) { throw new RuntimeException("stub"); }
    public static Parcel obtain() { throw new RuntimeException("stub"); }
    public static Parcel obtain(IBinder binder) { throw new RuntimeException("stub"); }
    public final void recycle() { throw new RuntimeException("stub"); }
    public final void writeInterfaceToken(String interfaceName) { throw new RuntimeException("stub"); }
    public final void enforceInterface(String interfaceName) { throw new RuntimeException("stub"); }
    public final void writeString(String val) { throw new RuntimeException("stub"); }
    public final String readString() { throw new RuntimeException("stub"); }
    public final void writeInt(int val) { throw new RuntimeException("stub"); }
    public final int readInt() { throw new RuntimeException("stub"); }
    public final void writeBoolean(boolean val) { throw new RuntimeException("stub"); }
    public final boolean readBoolean() { throw new RuntimeException("stub"); }
    public final void writeNoException() { throw new RuntimeException("stub"); }
    public void enforceNoDataAvail() { throw new RuntimeException("stub"); }
    public final void readException() { throw new RuntimeException("stub"); }
    public final void writeStrongInterface(IInterface val) { throw new RuntimeException("stub"); }
    public final IBinder readStrongBinder() { throw new RuntimeException("stub"); }
    public final void writeStrongBinder(IBinder val) { throw new RuntimeException("stub"); }
    public final String[] createStringArray() { throw new RuntimeException("stub"); }
    public final void writeStringArray(String[] val) { throw new RuntimeException("stub"); }
    public final <T extends Parcelable> void writeTypedObject(T val, int parcelableFlags) { throw new RuntimeException("stub"); }
    public final <T> T readTypedObject(Parcelable.Creator<T> c) { throw new RuntimeException("stub"); }
}
