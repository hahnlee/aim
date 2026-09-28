package android.os;

import java.util.ArrayList;
import java.util.List;

/** Test-only Parcel implementing just the AIDL operations exercised here. */
public final class Parcel {
    private final List<Object> values = new ArrayList<Object>();
    private int position;
    private boolean noException;

    public static Parcel obtain() {
        return new Parcel();
    }

    public void writeInterfaceToken(String descriptor) {
        values.add(descriptor);
    }

    public void enforceInterface(String descriptor) {
        Object actual = readValue();
        if (!descriptor.equals(actual)) {
            throw new SecurityException("wrong interface token: " + actual);
        }
    }

    public void enforceNoDataAvail() {
        if (position != values.size()) {
            throw new IllegalStateException("Parcel data not fully consumed");
        }
    }

    public void writeInt(int value) {
        values.add(Integer.valueOf(value));
    }

    public int readInt() {
        return ((Integer) readValue()).intValue();
    }

    public void writeLong(long value) {
        values.add(Long.valueOf(value));
    }

    public long readLong() {
        return ((Long) readValue()).longValue();
    }

    public void writeFloat(float value) {
        values.add(Float.valueOf(value));
    }

    public float readFloat() {
        return ((Float) readValue()).floatValue();
    }

    public void writeString(String value) {
        values.add(value);
    }

    public void writeObject(Object value) {
        values.add(value);
    }

    public Object readObject() {
        return readValue();
    }

    public void writeStrongBinder(IBinder value) {
        values.add(value);
    }

    public IBinder readStrongBinder() {
        return (IBinder) readValue();
    }

    public void recycle() {}

    public String readString() {
        return (String) readValue();
    }

    public void writeNoException() {
        noException = true;
    }

    public boolean hasNoException() {
        return noException;
    }

    public void writeBoolean(boolean value) {
        values.add(Boolean.valueOf(value));
    }

    public <T extends Parcelable> void writeTypedObject(T value, int flags) {
        if (value == null) {
            writeInt(0);
        } else {
            writeInt(1);
            value.writeToParcel(this, flags);
        }
    }

    public <T extends Parcelable> T readTypedObject(Parcelable.Creator<T> creator) {
        return readInt() == 0 ? null : creator.createFromParcel(this);
    }

    public <T extends Parcelable> void writeTypedArray(T[] values, int flags) {
        if (values == null) {
            writeInt(-1);
            return;
        }
        writeInt(values.length);
        for (T value : values) writeTypedObject(value, flags);
    }

    @SuppressWarnings("unchecked")
    public <T extends Parcelable> T[] createTypedArray(Parcelable.Creator<T> creator) {
        int length = readInt();
        if (length < 0) return null;
        T[] values = creator.newArray(length);
        for (int i = 0; i < length; i++) values[i] = readTypedObject(creator);
        return values;
    }

    public boolean readBoolean() {
        return ((Boolean) readValue()).booleanValue();
    }

    public int dataAvail() {
        return values.size() - position;
    }

    public void setDataPosition(int value) {
        if (value < 0 || value > values.size()) throw new IllegalArgumentException("position");
        position = value;
    }

    private Object readValue() {
        if (position >= values.size()) throw new IllegalStateException("Parcel underflow");
        return values.get(position++);
    }
}
