package android.os;

/** Android's little-endian, four-byte-aligned scalar parcel boundary. */
public class Parcel {
    private final java.io.ByteArrayOutputStream bytes = new java.io.ByteArrayOutputStream();
    public static Parcel obtain() { return new Parcel(); }
    public void writeInt(int value) {
        for (int i = 0; i < 4; i++) bytes.write(value >>> (i * 8));
    }
    public void writeBoolean(boolean value) { writeInt(value ? 1 : 0); }
    private void string(String value, java.nio.charset.Charset charset, int terminator) {
        writeInt(value == null ? -1 : value.getBytes(charset).length / terminator);
        if (value == null) return;
        bytes.writeBytes(value.getBytes(charset));
        for (int i = 0; i < terminator; i++) bytes.write(0);
        while (bytes.size() % 4 != 0) bytes.write(0);
    }
    public void writeString(String value) { string(value, java.nio.charset.StandardCharsets.UTF_16LE, 2); }
    public void writeString8(String value) { string(value, java.nio.charset.StandardCharsets.UTF_8, 1); }
    public byte[] marshall() { return bytes.toByteArray(); }
    public void recycle() {}
}
