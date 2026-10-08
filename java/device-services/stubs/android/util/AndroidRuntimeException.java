// Compile-only original framework exception API.
package android.util;
public class AndroidRuntimeException extends RuntimeException {
    public AndroidRuntimeException() { throw new RuntimeException("stub"); }
    public AndroidRuntimeException(String message) { super(message); throw new RuntimeException("stub"); }
}
