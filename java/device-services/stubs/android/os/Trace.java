// Compile-only pinned image API.
package android.os;
public final class Trace {
    private Trace(){throw new RuntimeException("stub");}
    public static final long TRACE_TAG_PACKAGE_MANAGER=1L<<18;
    public static void traceBegin(long tag,String name){throw new RuntimeException("stub");}
    public static void traceEnd(long tag){throw new RuntimeException("stub");}
}
