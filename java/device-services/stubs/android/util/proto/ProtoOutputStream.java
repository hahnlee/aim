// Compile-only original image type; no runtime implementation.
package android.util.proto;
public class ProtoOutputStream extends ProtoStream {
    public ProtoOutputStream() { throw new RuntimeException("stub"); }
    public long start(long field) { throw new RuntimeException("stub"); }
    public void end(long token) { throw new RuntimeException("stub"); }
    public void write(long field, long value) { throw new RuntimeException("stub"); }
    public void write(long field, int value) { throw new RuntimeException("stub"); }
    public void write(long field, boolean value) { throw new RuntimeException("stub"); }
    public void write(long field, String value) { throw new RuntimeException("stub"); }
    public void write(long field, byte[] value) { throw new RuntimeException("stub"); }
    public void write(long field, double value) { throw new RuntimeException("stub"); }
}
