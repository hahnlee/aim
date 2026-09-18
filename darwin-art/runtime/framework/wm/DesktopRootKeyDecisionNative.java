package dev.darwinart.runtime.wm;

import android.os.IBinder;

/**
 * Narrow native owner for one exact desktop-root decision admission.  The
 * boundary carries an already retained root target and never performs Java
 * policy, Binder identity lookup, or input readiness decisions.
 */
final class DesktopRootKeyDecisionNative {
    static final int RETAINED = 0;
    static final int RETRY = 1;
    static final int FAILED = 2;

    interface Boundary {
        long attach(long binding, long nativeTarget, long incarnation, IBinder capability);
        int publish(long binding, DesktopRootFocusDecision decision);
        void close(long binding);
    }

    static final Boundary INSTANCE = new Boundary() {
        @Override public long attach(long binding, long nativeTarget, long incarnation,
                IBinder capability) {
            return nativeAttach(binding, nativeTarget, incarnation, capability);
        }

        @Override public int publish(long binding, DesktopRootFocusDecision decision) {
            if (decision == null) throw new IllegalArgumentException("decision is null");
            return nativePublish(binding, decision.incarnation, decision.factSerial,
                    decision.sequence, decision.epoch, decision.originalChannelToken);
        }

        @Override public void close(long binding) { nativeClose(binding); }
    };

    private DesktopRootKeyDecisionNative() {}

    private static native long nativeAttach(long binding, long nativeTarget,
            long incarnation, IBinder capability);
    private static native int nativePublish(long binding, long incarnation,
            long factSerial, long sequence, long epoch, IBinder originalChannelToken);
    private static native void nativeClose(long binding);
}
