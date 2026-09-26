package android.os;

import java.util.List;

/** Test stub recording what a reply carries. */
public final class Parcel {
    public boolean noException;
    public Object typed;
    public boolean listWritten;

    public void writeNoException() { noException = true; }
    public void writeTypedObject(Parcelable value, int flags) { typed = value; }
    public <T> void writeTypedList(List<T> value, int flags) { listWritten = true; typed = value; }
    public int readInt() { return 0; }
    public void enforceNoDataAvail() {}
}
