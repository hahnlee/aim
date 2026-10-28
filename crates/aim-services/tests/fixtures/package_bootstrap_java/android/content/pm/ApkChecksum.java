package android.content.pm;
public record ApkChecksum(String split, int type, byte[] value) {
    public int getType() { return type; }
    public byte[] getValue() { return value; }
}
