package android.os;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

public final class Parcel {
    private final List<Object> values = new ArrayList<Object>();
    private int position;
    private boolean noException;
    public static Parcel obtain() { return new Parcel(); }
    public void writeInterfaceToken(String descriptor) { values.add(descriptor); }
    public void enforceInterface(String descriptor) {
        Object actual = readValue();
        if (!descriptor.equals(actual)) throw new SecurityException("wrong token");
    }
    public void enforceNoDataAvail() {
        if (position != values.size()) throw new IllegalStateException("trailing parcel data");
    }
    public void writeString(String value) { values.add(value); }
    public String readString() { return (String) readValue(); }
    public void writeInt(int value) { values.add(Integer.valueOf(value)); }
    public int readInt() { return ((Integer) readValue()).intValue(); }
    public void writeStrongBinder(IBinder value) { values.add(value); }
    public IBinder readStrongBinder() { return (IBinder) readValue(); }
    public <T extends Parcelable> void writeTypedObject(T value, int flags) { values.add(value); }
    @SuppressWarnings("unchecked")
    public <T> T readTypedObject(Parcelable.Creator<T> creator) { return (T) readValue(); }
    public void writeNoException() { noException = true; }
    public boolean hasNoException() { return noException; }
    public void writeIntArray(int[] value) { values.add(value); }
    public int[] readIntArray() { return (int[]) readValue(); }
    public void writeBoolean(boolean value) { values.add(Boolean.valueOf(value)); }
    public boolean readBoolean() { return ((Boolean) readValue()).booleanValue(); }
    public void writeByteArray(byte[] value) { values.add(value); }
    public void writeMap(Map<?, ?> value) { values.add(value); }
    public void writeTypedList(List<?> value) { values.add(value); }
    public int dataAvail() { return values.size() - position; }
    public void recycle() {}
    private Object readValue() {
        if (position >= values.size()) throw new IllegalStateException("parcel underflow");
        return values.get(position++);
    }
}
