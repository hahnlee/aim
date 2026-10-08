// Compile-only pinned image API; checked by the device-services build node.
package android.system;
public final class ErrnoException extends Exception {
    public ErrnoException(String functionName, int errno) { throw new RuntimeException("stub"); }
    public java.io.IOException rethrowAsIOException() throws java.io.IOException { throw new RuntimeException("stub"); }
 public int errno;
}
