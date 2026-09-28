package dalvik.system;

import java.io.IOException;

/** Compile-only declaration; core-libart owns the runtime class. */
public final class VMDebug {
    public static native int getMethodTracingMode();
    public static native void startMethodTracing(
            String traceFileName, int bufferSize, int flags,
            boolean samplingEnabled, int intervalUs);
    public static native void startMethodTracingDdms(
            int bufferSize, int flags, boolean samplingEnabled, int intervalUs);
    public static native void stopMethodTracing();
    public static native void setAllocTrackerStackDepth(int stackDepth);
    public static void attachAgent(String agent, ClassLoader classLoader) throws IOException {
        throw new AssertionError();
    }
}
