package libcore.util;

/** Compile-only Android bootclasspath surface for hidden ART run-tests. */
public class NativeAllocationRegistry {
    public static NativeAllocationRegistry createNonmalloced(
            ClassLoader classLoader, long freeFunction, long size) {
        throw new AssertionError();
    }

    public Runnable registerNativeAllocation(Object referent, long nativePtr) {
        throw new AssertionError();
    }
}
