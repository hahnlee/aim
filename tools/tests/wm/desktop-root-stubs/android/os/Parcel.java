package android.os;

import java.util.ArrayList;
import java.util.List;

public final class Parcel {
    private final List<Object> values = new ArrayList<Object>();
    private int position;

    public static Parcel obtain() { return new Parcel(); }
    public void recycle() {}
    public void writeInterfaceToken(String value) { values.add(value); }
    public void enforceInterface(String expected) {
        if (!expected.equals(read())) throw new SecurityException("wrong descriptor");
    }
    public void enforceNoDataAvail() {
        if (position != values.size()) throw new IllegalStateException("trailing parcel data");
    }
    public void writeLong(long value) { values.add(Long.valueOf(value)); }
    public long readLong() { return ((Long) read()).longValue(); }
    public void writeInt(int value) { values.add(Integer.valueOf(value)); }
    public int readInt() { return ((Integer) read()).intValue(); }
    public void writeBoolean(boolean value) { values.add(Boolean.valueOf(value)); }
    public boolean readBoolean() { return ((Boolean) read()).booleanValue(); }
    public void writeStrongBinder(IBinder value) { values.add(value); }
    public IBinder readStrongBinder() { return (IBinder) read(); }
    public void writeNoException() { values.add(Integer.valueOf(0)); }
    public void readException() { read(); }
    private Object read() {
        if (position >= values.size()) throw new IllegalStateException("parcel underflow");
        return values.get(position++);
    }
}
