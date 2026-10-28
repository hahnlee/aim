package android.os;
public class Parcel {
    public static Object[] written;
    public static int recycled;
    public static Parcel obtain() { return new Parcel(); }
    public <T> void writeTypedList(java.util.List<T> list) { written = list.toArray(); }
    public <T> void writeTypedArray(T[] array, int flags) { written = array; }
    public byte[] marshall() { return new byte[] {(byte) written.length}; }
    public void recycle() { recycled++; }
}
