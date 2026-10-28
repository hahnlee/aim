// Compile-only pinned image API; never packaged with the guest.
package android.os;
public class ServiceSpecificException extends RuntimeException {
    public final int errorCode = 0;
    public ServiceSpecificException(int code, String message) { throw new RuntimeException("stub"); }
}
