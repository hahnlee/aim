package org.apache.harmony.dalvik.ddmc;

/** Compile-only declaration; core-libart owns the runtime class. */
public final class DdmVmInternal {
    public static native void setThreadNotifyEnabled(boolean enabled);
    public static native byte[] getThreadStats();
    public static native StackTraceElement[] getStackTraceById(int threadId);
    public static native void setRecentAllocationsTrackingEnabled(boolean enabled);
}
