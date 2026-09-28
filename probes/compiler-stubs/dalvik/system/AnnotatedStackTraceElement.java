package dalvik.system;

/**
 * Compile-time-only signature view of Android's boot class.
 *
 * <p>The class is never packaged into a probe or application DEX. Runtime
 * resolution always targets the pinned Android 16 core-libart definition.</p>
 */
public final class AnnotatedStackTraceElement {
    public StackTraceElement getStackTraceElement() {
        throw new AssertionError("compile-time stub");
    }

    public Object[] getHeldLocks() {
        throw new AssertionError("compile-time stub");
    }

    public Object getBlockedOn() {
        throw new AssertionError("compile-time stub");
    }
}
