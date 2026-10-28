// Host-only Android boundary double; production policy is compiled unchanged.
package android.content.pm;
public class Signature {
    private final byte[] bytes;
    public Signature(byte[] bytes) { this.bytes=bytes; }
    public byte[] toByteArray() { return bytes; }
    @Override public int hashCode() { return java.util.Arrays.hashCode(bytes); }
}
