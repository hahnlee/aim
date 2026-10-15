// Compile-only image ABI, not included at runtime.
// Compile-only pinned original API; verified by the device-services image linkage gate.
package android.util;
public class ExceptionUtils {
    public ExceptionUtils() { throw new RuntimeException("stub"); }
    public static RuntimeException propagate(Throwable throwable) { throw new RuntimeException("stub"); }
}
