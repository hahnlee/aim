package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.RemoteException;
import android.util.Log;

/** Process-owned WMS decision retention and exact native-root admission. */
final class DesktopRootFocusDecisionClient extends DesktopRootFocusDecisionTransport.Receiver {
    private final long incarnation;
    private final DesktopRootKeyDecisionNative.Boundary nativeBoundary;
    private DesktopRootFocusDecision latest;
    private boolean closed;
    private IBinder serverCapability;
    private boolean serverLinked;
    private boolean linking;
    private long nativeBinding;
    private long nativeTarget;
    private boolean nativeAttaching;
    private final IBinder.DeathRecipient serverDeath = new IBinder.DeathRecipient() {
        @Override public void binderDied() { closeAdmission(); }
    };

    DesktopRootFocusDecisionClient(long incarnation) {
        this(incarnation, DesktopRootKeyDecisionNative.INSTANCE);
    }

    DesktopRootFocusDecisionClient(long incarnation,
            DesktopRootKeyDecisionNative.Boundary boundary) {
        if (incarnation == 0) throw new IllegalArgumentException("missing root incarnation");
        if (boundary == null) throw new IllegalArgumentException("missing native boundary");
        this.incarnation = incarnation;
        this.nativeBoundary = boundary;
    }

    void attach(IBinder capability, long target) throws RemoteException {
        if (capability == null) throw new IllegalArgumentException("missing server capability");
        if (target == 0) throw new IllegalArgumentException("missing native root target");
        final long priorBinding;
        synchronized (this) {
            if (closed) throw new RemoteException("desktop root decision admission closed");
            if (serverCapability != null && serverCapability != capability)
                throw new RemoteException("desktop root server capability changed");
            if (nativeAttaching)
                throw new RemoteException("desktop root native attachment in progress");
            nativeAttaching = true;
            priorBinding = nativeBinding;
        }

        long candidateBinding = 0;
        try {
            // Capture the exact native root before the trusted Binder
            // handshake and its synchronous early-decision drain.
            candidateBinding = nativeBoundary.attach(priorBinding, target, incarnation, capability);
            if (candidateBinding == 0)
                throw new RemoteException("desktop root native decision attach rejected");
        } catch (RemoteException | RuntimeException | LinkageError failure) {
            synchronized (this) { nativeAttaching = false; }
            throw failure;
        }

        boolean discard = false;
        synchronized (this) {
            nativeAttaching = false;
            if (closed) {
                discard = true;
            } else if (nativeBinding == 0) {
                nativeBinding = candidateBinding;
                nativeTarget = target;
            } else if (nativeBinding != candidateBinding) {
                discard = true;
            }
        }
        if (discard) {
            nativeBoundary.close(candidateBinding);
            throw new RemoteException("desktop root native attachment superseded");
        }

        observeServerLifetime(capability);
        DesktopRootProtocol.ServerIdentity identity =
                DesktopRootProtocol.attachFocusDecisions(capability, this);
        if (!pinServer(identity.pid, identity.uid)) {
            closeAdmission();
            throw new RemoteException("desktop root server identity changed");
        }
    }

    private void observeServerLifetime(IBinder capability) throws RemoteException {
        synchronized (this) {
            if (closed) throw new RemoteException("desktop root decision admission closed");
            if (serverCapability != null && serverCapability != capability)
                throw new RemoteException("desktop root server capability changed");
            if (serverLinked) return;
            if (linking) throw new RemoteException("desktop root lifetime attachment in progress");
            serverCapability = capability;
            linking = true;
        }
        boolean linked = false;
        boolean retained = false;
        try {
            capability.linkToDeath(serverDeath, 0);
            linked = true;
            synchronized (this) {
                retained = !closed;
                if (retained) serverLinked = true;
            }
        } catch (RemoteException | RuntimeException failure) {
            closeAdmission();
            throw failure;
        } finally {
            synchronized (this) { linking = false; }
            if (linked && !retained) unlinkServer(capability);
        }
        if (!retained) throw new RemoteException("desktop root server died during attachment");
    }

    @Override protected void onDecision(DesktopRootFocusDecision decision) {
        if (decision == null || decision.incarnation != incarnation)
            throw new SecurityException("decision belongs to another root incarnation");
        final long binding;
        final DesktopRootFocusDecision publishDecision;
        synchronized (this) {
            if (closed) return;
            if (latest != null) {
                if (decision.sequence < latest.sequence) return;
                if (decision.sequence == latest.sequence && !decision.equals(latest))
                    throw new IllegalArgumentException("decision sequence reused with a different value");
            }
            // Keep the first immutable Java record as the snapshot identity;
            // an equal wire retry is published to native but does not replace it.
            publishDecision = latest != null && decision.sequence == latest.sequence
                    ? latest : decision;
            latest = publishDecision;
            binding = nativeBinding;
        }
        if (binding == 0) {
            closeAdmission();
            throw new IllegalStateException("decision arrived before native attachment");
        }
        final int result;
        try {
            result = nativeBoundary.publish(binding, publishDecision);
        } catch (RuntimeException | LinkageError failure) {
            // Publishing can synchronously re-enter this client.  A newer
            // record may have won while the native owner was invoking its
            // callback; an old failed attempt must not close that admission.
            if (!closeIfCurrent(publishDecision, binding)) return;
            throw failure;
        }
        if (result == DesktopRootKeyDecisionNative.RETRY) {
            if (!isCurrentAttempt(publishDecision, binding)) return;
            throw new IllegalStateException("native key decision requested retry");
        }
        if (result != DesktopRootKeyDecisionNative.RETAINED) {
            if (!closeIfCurrent(publishDecision, binding)) return;
            throw new IllegalStateException("native key decision rejected");
        }
    }

    private synchronized boolean isCurrentAttempt(DesktopRootFocusDecision attempted,
            long binding) {
        return !closed && nativeBinding == binding && latest == attempted
                && latest.sequence == attempted.sequence;
    }

    /**
     * Atomically claims terminal closure for one native publish attempt. A
     * superseding callback, or a concurrent close, wins without disturbing
     * the newer admission. All callback-capable cleanup is performed after
     * releasing this object's monitor.
     */
    private boolean closeIfCurrent(DesktopRootFocusDecision attempted, long binding) {
        IBinder unlink;
        long closingBinding;
        synchronized (this) {
            if (!isCurrentAttemptLocked(attempted, binding)) return false;
            closed = true;
            latest = null;
            unlink = serverLinked ? serverCapability : null;
            serverLinked = false;
            serverCapability = null;
            closingBinding = nativeBinding;
            nativeBinding = 0;
            nativeTarget = 0;
        }
        finishClosure(unlink, closingBinding);
        return true;
    }

    private boolean isCurrentAttemptLocked(DesktopRootFocusDecision attempted, long binding) {
        return !closed && nativeBinding == binding && latest == attempted
                && latest.sequence == attempted.sequence;
    }

    synchronized DesktopRootFocusDecision snapshot() { return closed ? null : latest; }

    synchronized boolean isNativeBound() { return !closed && nativeBinding != 0; }

    void closeAdmission() {
        IBinder unlink;
        long binding;
        synchronized (this) {
            if (closed) return;
            closed = true;
            latest = null;
            unlink = serverLinked ? serverCapability : null;
            serverLinked = false;
            serverCapability = null;
            binding = nativeBinding;
            nativeBinding = 0;
            nativeTarget = 0;
        }
        finishClosure(unlink, binding);
    }

    private void finishClosure(IBinder unlink, long binding) {
        closeTransportAdmission();
        // Stop native decision admission before entering Binder lifetime
        // cleanup; unlink may synchronously re-enter the process.
        try {
            if (binding != 0) nativeBoundary.close(binding);
        } finally {
            if (unlink != null) unlinkServer(unlink);
        }
    }

    private void unlinkServer(IBinder capability) {
        try {
            capability.unlinkToDeath(serverDeath, 0);
        } catch (RuntimeException failure) {
            Log.w("DesktopRootDecisions", "server lifetime unlink failed after closure", failure);
        }
    }
}
