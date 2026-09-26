package android.os;

import java.util.ArrayList;
import java.util.List;

public final class Parcel {
    private final List<Object> values = new ArrayList<Object>();
    private int position;
    private int writesBeforeFailure = -1;

    public static Parcel obtain() { return new Parcel(); }

    public void failNextWrite() { writesBeforeFailure = 0; }
    private void write(Object value) {
        if (writesBeforeFailure == 0) throw new RuntimeException("injected reply write failure");
        if (writesBeforeFailure > 0) --writesBeforeFailure;
        values.add(value);
    }

    public void recycle() {}
    public void writeInterfaceToken(String token) { write(token); }
    public String readInterfaceToken() { return (String) read(); }
    public void enforceInterface(String expected) {
        Object actual = read();
        if (!expected.equals(actual)) throw new SecurityException("bad interface token");
    }
    public void enforceNoDataAvail() {
        if (position != values.size()) throw new IllegalStateException("trailing parcel data");
    }
    public void writeStrongBinder(IBinder binder) { write(binder); }
    public IBinder readStrongBinder() { return (IBinder) read(); }
    public void writeInt(int value) { write(Integer.valueOf(value)); }
    public int readInt() { return ((Integer) read()).intValue(); }
    public void writeLong(long value) { write(Long.valueOf(value)); }
    public long readLong() { return ((Long) read()).longValue(); }
    public void writeString(String value) { write(value); }
    public String readString() { return (String) read(); }
    public void writeBoolean(boolean value) { write(Boolean.valueOf(value)); }
    public void writeFloat(float value) { write(Float.valueOf(value)); }
    public void writeStrongInterface(Object value) { write(value); }
    public boolean readBoolean() { return ((Boolean) read()).booleanValue(); }
    public void writeFloatArray(float[] value) { write(value); }
    public float[] createFloatArray() { return (float[]) read(); }
    public <T extends Parcelable> void writeTypedObject(T value, int flags) { write(value); }
    @SuppressWarnings("unchecked")
    public <T> T readTypedObject(Parcelable.Creator<T> creator) { return (T) read(); }
    public void writeNoException() { write(Integer.valueOf(0)); }
    public void readException() { read(); }
    public void writeParcelable(Parcelable value, int flags) { write(value); }

    private Object read() {
        if (position >= values.size()) throw new IllegalStateException("parcel underflow");
        return values.get(position++);
    }
}
