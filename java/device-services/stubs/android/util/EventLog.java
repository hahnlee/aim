// Compile-only pinned image API; no runtime implementation.
package android.util;
public class EventLog {
    public EventLog() { throw new RuntimeException("stub"); }
    public static int writeEvent(int tag, Object... list) { throw new RuntimeException("stub"); }
}
