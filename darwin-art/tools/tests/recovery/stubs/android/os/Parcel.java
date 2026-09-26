package android.os;

import java.util.ArrayDeque;

/** Test stub: a value queue. */
public final class Parcel {
    private final ArrayDeque<Object> values = new ArrayDeque<>();
    public boolean noException;

    public void writeInterfaceToken(String token) { values.add(token); }
    public void enforceInterface(String token) {
        if (!token.equals(values.poll())) throw new SecurityException("wrong interface");
    }
    public void writeString(String value) { values.add(value == null ? "" : value); }
    public String readString() { return (String) values.poll(); }
    public void writeStrongBinder(IBinder value) { values.add(value == null ? "" : value); }
    public IBinder readStrongBinder() { Object v = values.poll(); return v instanceof IBinder ? (IBinder) v : null; }
    public void writeBoolean(boolean value) { values.add(value); }
    public boolean readBoolean() { return (Boolean) values.poll(); }
    public void writeInt(int value) { values.add(value); }
    public int readInt() { return (Integer) values.poll(); }
    public <T> T readTypedObject(Object creator) { values.poll(); return null; }
    public void writeNoException() { noException = true; }
    public void enforceNoDataAvail() {
        if (!values.isEmpty()) throw new IllegalStateException("trailing data");
    }
}
