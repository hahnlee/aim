package dev.darwinart.runtime.wm;

import android.view.InputChannel;

/** JNI ABI fixture only: actual client Handler/Binder behavior is tested separately. */
public final class DesktopRootClient {
    private static native long acquireTarget();
    private static native long targetIncarnation(long target);
    private static native boolean observe(long target, DesktopRootClient callback);
    private static native void releaseTarget(long target);
    public static int facts;
    public static boolean closed;
    public static int rejections;
    public static long capturedTarget;
    public static boolean ensureAttached(InputChannel channel) {
        if (channel == null) throw new AssertionError("missing input channel");
        long target = acquireTarget();
        if (target == 0 || targetIncarnation(target) == 0) throw new AssertionError();
        capturedTarget = target;
        if (!observe(target, new DesktopRootClient())) throw new AssertionError();
        return true;
    }
    public void onHostFact(int kind, long incarnation, long serial, boolean key) {
        if (closed || kind != 2 || incarnation == 0 || serial == 0 || key)
            throw new AssertionError("invalid native fact");
        ++facts;
    }
    public void onObservationRejected(long incarnation) {
        if (closed || incarnation == 0) throw new AssertionError();
        ++rejections;
    }
    public static void closeAdmission() { closed = true; }
    public static boolean isQuiesced() { return closed; }
}
