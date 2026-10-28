package android.os;
import java.io.ByteArrayOutputStream;
public final class Parcel {
    private final ByteArrayOutputStream out = new ByteArrayOutputStream();
    public static Parcel obtain() { return new Parcel(); }
    public <T extends Parcelable> void writeTypedObject(T value, int flags) {
        writeInt(value == null ? 0 : 1);
        if (value != null) value.writeToParcel(this, flags);
    }
    public void writeInt(int value) { for (int i = 0; i < 4; i++) out.write(value >>> (8 * i)); }
    public byte[] marshall() { return out.toByteArray(); }
    public void recycle() {}
}
