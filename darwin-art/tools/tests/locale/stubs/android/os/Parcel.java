package android.os;

import java.util.ArrayList;
import java.util.List;

/** Test-only Parcel implementing the ILocaleManager typed-object wire. */
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

    public void writeString(String value) {
        values.add(value);
    }

    public String readString() {
        return (String) readValue();
    }

    public void writeNoException() {
        noException = true;
    }

    public boolean hasNoException() {
        return noException;
    }

    public void writeTypedObject(Parcelable value, int flags) {
        values.add(value);
    }

    public <T> T readTypedObject(Parcelable.Creator<T> creator) {
        return creator.createFromParcel(this);
    }

    public Object readValue() {
        if (position >= values.size()) throw new IllegalStateException("Parcel underflow");
        return values.get(position++);
    }

    public int dataAvail() {
        return values.size() - position;
    }
}
