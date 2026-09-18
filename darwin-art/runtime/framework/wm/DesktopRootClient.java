package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.SystemClock;
import android.util.Log;
import android.view.InputChannel;
import java.util.ArrayList;
import java.util.Collections;
import java.util.IdentityHashMap;
import java.util.LinkedHashSet;
import java.util.Set;
import java.util.WeakHashMap;

/**
 * Process-owned bridge from the exact AppKit desktop root to the WMS owner.
 *
 * <p>This class deliberately has no focus policy.  The native observer only
 * schedules host facts; every Binder fact is sent from the process main
 * looper after the native callback has returned.</p>
 */
public final class DesktopRootClient {
    private static final String TAG = "DarwinDesktopRoot";
    private static final String SERVICE_NAME = "darwin.desktop_root";
    private static final int FACT_ACTIVATED = 1;
    private static final int FACT_RESIGNED = 2;
    private static final int FACT_CLOSED = 3;

    private static final Object LOCK = new Object();
    private static final Object QUEUE_TOKEN = new Object();
    private static Handler mainHandler;
    private static final Set<Work> PENDING = Collections.newSetFromMap(
            new IdentityHashMap<Work, Boolean>());
    private static final Set<IBinder> BIND_PENDING = new LinkedHashSet<>();
    private static final Set<IBinder> BIND_DONE = Collections.newSetFromMap(new WeakHashMap<IBinder, Boolean>());
    private static boolean bindScheduled;
    private static long bindRequestGeneration;

    private enum State { IDLE, ATTACH_QUEUED, ATTACHING, OBSERVING, ATTACHED, CLOSING }

    private static State state = State.IDLE;
    private static DesktopRootClient singleton;
    private static boolean admissionClosed;
    private static int running;
    private static long lastSerial;
    private static boolean haveSerial;

    /* The registration is intentionally retained across an observe rejection.
     * DesktopRootEndpoint has no unregister transaction; retrying REGISTER
     * would create a duplicate capability for the same attachment. */
    private static IBinder registeredCapability;
    private static Binder registeredLifetime;
    private static long registeredIncarnation;
    private static DesktopRootFocusDecisionClient registeredDecisions;
    private long attemptTarget;

    private DesktopRootClient() {}

    /** Native handle acquisition retains the exact published AppKit root. */
    private static native long acquireTarget();
    private static native long targetIncarnation(long target);
    /** Returns whether the native Bind was queued, not whether focus attached. */
    private static native boolean observe(long target, DesktopRootClient callback);
    private static native void releaseTarget(long target);

    /**
     * Coalesces process receiver calls and schedules attachment on the real
     * main looper.  A true result means the request is queued or already
     * owned; it does not claim root binding or Android focus completion.
     */
    public static boolean ensureAttached(InputChannel originalChannel) {
        if (originalChannel == null) throw new IllegalArgumentException("missing window input channel");
        final IBinder originalToken = originalChannel.getToken();
        if (originalToken == null) throw new IllegalStateException("window channel has no original token");
        synchronized (LOCK) {
            if (admissionClosed) {
                Log.e(TAG, "attachment rejected after shutdown admission closed");
                return false;
            }
            if (state == State.CLOSING) return false;
            if (!BIND_DONE.contains(originalToken)) {
                BIND_PENDING.add(originalToken);
                ++bindRequestGeneration; // Every explicit retry is an intent.
            }
            if (state != State.IDLE) {
                return state != State.ATTACHED && state != State.OBSERVING
                        || scheduleBindingsLocked();
            }
            // A callback object is never reused: a late callback from a
            // failed attempt can therefore not revoke a successor target.
            singleton = new DesktopRootClient();
            if (mainHandler == null) {
                Looper mainLooper = Looper.getMainLooper();
                if (mainLooper == null) {
                    state = State.IDLE;
                    Log.e(TAG, "main looper unavailable for desktop root attachment");
                    return false;
                }
                try {
                    mainHandler = new Handler(mainLooper);
                } catch (RuntimeException | LinkageError failure) {
                    state = State.IDLE;
                    Log.e(TAG, "main handler creation failed", failure);
                    return false;
                }
            }
            state = State.ATTACH_QUEUED;
            if (!postLocked(new Work(new Runnable() {
                @Override public void run() { attachOnMain(); }
            }))) {
                state = State.IDLE;
                Log.e(TAG, "main-looper attachment scheduling rejected");
                return false;
            }
            return true;
        }
    }

    /**
     * Called by the process shutdown owner before it asks the native side to
     * unbind and clear JNI globals.  It never waits on a stopped looper.
     */
    public static void closeAdmission() {
        final DesktopRootFocusDecisionClient decisions;
        synchronized (LOCK) {
            if (admissionClosed) return;
            admissionClosed = true;
            state = State.CLOSING;
            if (mainHandler != null) mainHandler.removeCallbacksAndMessages(QUEUE_TOKEN);
            PENDING.clear();
            BIND_PENDING.clear();
            BIND_DONE.clear();
            bindScheduled = false;
            decisions = registeredDecisions;
            if (decisions != null) ++running;
        }
        // Lifetime cleanup can enter Binder transport. Do not hold LOCK,
        // and include this admitted closure tail in process quiescence.
        if (decisions != null) {
            try { decisions.closeAdmission(); }
            finally { synchronized (LOCK) { --running; } }
        }
    }

    /** True only after closeAdmission and all queued/running Java work drain. */
    public static boolean isQuiesced() {
        synchronized (LOCK) {
            return admissionClosed && PENDING.isEmpty() && running == 0;
        }
    }

    /** Immutable callback entry point used by the native AppKit observer. */
    public void onHostFact(int kind, long incarnation, long serial, boolean keySnapshot) {
        if (!validFact(kind, incarnation, serial, keySnapshot)) {
            Log.e(TAG, "invalid host fact rejected kind=" + kind);
            return;
        }
        final Event event = new Event(kind, incarnation, serial, keySnapshot);
        boolean posted;
        final long expectedTarget;
        synchronized (LOCK) {
            if (admissionClosed || singleton != this
                    || (state != State.OBSERVING && state != State.ATTACHED)
                    || registeredIncarnation != incarnation || attemptTarget == 0) {
                Log.w(TAG, "host fact rejected outside observation admission");
                return;
            }
            expectedTarget = attemptTarget;
            Work work = new Work(new Runnable() {
                @Override public void run() { processOnMain(event); }
            });
            posted = postLocked(work);
        }
        if (!posted) {
            Log.e(TAG, "host fact main-looper post rejected");
            abandonObservation(incarnation, expectedTarget);
        }
    }

    /** Native Bind failure callback; only schedules an exact-attempt close. */
    public void onObservationRejected(long incarnation) {
        requireIdentity(incarnation, "rejected desktop root incarnation");
        final long expectedTarget;
        synchronized (LOCK) {
            if (admissionClosed || singleton != this
                    || (state != State.OBSERVING && state != State.ATTACHED)
                    || registeredIncarnation != incarnation || attemptTarget == 0) return;
            expectedTarget = attemptTarget;
        }
        Work work = new Work(new Runnable() {
            @Override public void run() { abandonObservation(incarnation, expectedTarget); }
        });
        boolean posted;
        synchronized (LOCK) { posted = postLocked(work); }
        if (!posted) {
            Log.e(TAG, "observation rejection main-looper post rejected");
            abandonObservation(incarnation, expectedTarget);
        }
    }

    private static void attachOnMain() {
        synchronized (LOCK) {
            if (admissionClosed || state != State.ATTACH_QUEUED) return;
            state = State.ATTACHING;
        }

        DesktopRootClient client;
        synchronized (LOCK) { client = singleton; }
        long candidate = 0;
        boolean registerAttempted = false;
        boolean keepCandidate = false;
        boolean candidateRecorded = false;
        try {
            candidate = nativeBoundary().acquireTarget();
            if (candidate == 0) throw new IllegalStateException("desktop root unavailable");
            synchronized (LOCK) {
                if (singleton != client || state != State.ATTACHING) {
                    throw new IllegalStateException("desktop root attempt superseded");
                }
                // Record ownership immediately so pre-REGISTER failures also
                // release the exact retained native root.
                client.attemptTarget = candidate;
                candidateRecorded = true;
            }
            long incarnation = nativeBoundary().targetIncarnation(candidate);
            requireIdentity(incarnation, "native desktop root incarnation");

            synchronized (LOCK) {
                if (registeredCapability != null && registeredIncarnation != incarnation) {
                    throw new IllegalStateException("desktop root incarnation changed before retry");
                }
            }

            if (registeredCapability == null) {
                IBinder service = ServiceManager.getService(SERVICE_NAME);
                if (service == null) throw new RemoteException("desktop root service unavailable");
                Binder lifetime = new Binder();
                registerAttempted = true;
                IBinder capability = DesktopRootProtocol.register(service, incarnation, lifetime);
                // Publish the exact capability before observe: native Bind may
                // synchronously queue its initial snapshot during this call.
                synchronized (LOCK) {
                    registeredCapability = capability;
                    registeredLifetime = lifetime;
                    registeredIncarnation = incarnation;
                }
            }

            final DesktopRootFocusDecisionClient decisions;
            final IBinder decisionCapability;
            final DesktopRootKeyDecisionNative.Boundary decisionBoundary =
                    nativeBoundary().keyDecisions();
            synchronized (LOCK) {
                if (admissionClosed) throw new IllegalStateException("shutdown admission closed");
                if (registeredDecisions == null)
                    registeredDecisions = new DesktopRootFocusDecisionClient(
                            registeredIncarnation, decisionBoundary);
                decisions = registeredDecisions;
                decisionCapability = registeredCapability;
            }
            // Reuse this process-owned callback after uncertain attach replies
            // and observation retries. Remote work never holds the client lock.
            decisions.attach(decisionCapability, candidate);

            // The exact canonical windows are bound before native observe can
            // emit its genuine initial host snapshot. This is not activation.
            long boundGeneration = bindWindowsOnMain();

            synchronized (LOCK) {
                if (admissionClosed) throw new IllegalStateException("shutdown admission closed");
                client.attemptTarget = candidate;
                state = State.OBSERVING;
            }
            if (!nativeBoundary().observe(candidate, client)) {
                throw new IllegalStateException("desktop root observation rejected");
            }
            synchronized (LOCK) {
                // Bind is asynchronous.  ATTACHED denotes an admitted,
                // scheduled observer only; WMS focus remains separate.
                if (singleton != client || state != State.OBSERVING
                        || client.attemptTarget != candidate) {
                    throw new IllegalStateException("desktop root observation attempt superseded");
                }
                state = admissionClosed ? State.CLOSING : State.ATTACHED;
                if (!admissionClosed && boundGeneration != bindRequestGeneration
                        && !scheduleBindingsLocked()) {
                    throw new IllegalStateException("desktop window binding tail scheduling rejected");
                }
                keepCandidate = true; // native shutdown owner releases it.
            }
        } catch (RemoteException | RuntimeException | LinkageError failure) {
            Log.e(TAG, "desktop root attachment failed closed", failure);
            synchronized (LOCK) {
                if (registerAttempted && registeredCapability == null) {
                    state = State.CLOSING;
                    Log.e(TAG, "desktop root REGISTER outcome uncertain; retry prohibited");
                } else if (singleton == client && client.attemptTarget == candidate) {
                    state = admissionClosed ? State.CLOSING : State.IDLE;
                }
            }
        } finally {
            if (!keepCandidate && candidate != 0) {
                if (candidateRecorded) releaseOwnedTarget(client, candidate);
                else releaseNative(candidate);
            }
        }
    }

    private static boolean scheduleBindingsLocked() {
        if (BIND_PENDING.isEmpty() || bindScheduled) return true;
        bindScheduled = true;
        if (postLocked(new Work(DesktopRootClient::bindWindowsOnMain))) return true;
        bindScheduled = false;
        return false;
    }

    private static long bindWindowsOnMain() {
        final ArrayList<IBinder> requests;
        final IBinder capability;
        final long generation;
        synchronized (LOCK) {
            bindScheduled = false;
            if (admissionClosed || state == State.CLOSING || state == State.IDLE
                    || registeredCapability == null) return bindRequestGeneration;
            requests = new ArrayList<>(BIND_PENDING);
            BIND_PENDING.removeAll(requests);
            capability = registeredCapability;
            generation = bindRequestGeneration;
        }
        for (IBinder original : requests) {
            try {
                DesktopRootProtocol.bindWindow(capability, original, 0);
                synchronized (LOCK) {
                    if (!admissionClosed && state != State.CLOSING
                            && registeredCapability == capability) BIND_DONE.add(original);
                }
            } catch (DesktopRootProtocol.BindingRejectedException rejected) {
                // A removed/stale window is not an outstanding delivery
                // obligation. Do not retain its Binder through window churn.
                synchronized (LOCK) { BIND_PENDING.remove(original); }
                Log.w(TAG, "desktop window binding terminally rejected");
            } catch (RemoteException | RuntimeException | LinkageError failure) {
                // Unlike REGISTER this exact tuple is idempotent. Retain an
                // uncertain request for a later explicit receiver/attach retry;
                // do not self-spin or claim unbound/committed from a lost ack.
                synchronized (LOCK) {
                    if (!admissionClosed && state != State.CLOSING) BIND_PENDING.add(original);
                }
                Log.e(TAG, "desktop window binding not acknowledged", failure);
            }
        }
        synchronized (LOCK) {
            if (!admissionClosed && state != State.CLOSING && generation != bindRequestGeneration
                    && !scheduleBindingsLocked())
                Log.e(TAG, "desktop window binding tail post rejected; pending requests retained");
        }
        return generation;
    }

    private void processOnMain(Event event) {
        IBinder endpoint;
        long expectedTarget;
        synchronized (LOCK) {
            if (admissionClosed || (state != State.OBSERVING && state != State.ATTACHED)
                    || singleton != this || registeredCapability == null
                    || attemptTarget == 0 || registeredIncarnation != event.incarnation) {
                Log.w(TAG, "host fact dropped after attachment state changed");
                return;
            }
            if (haveSerial && Long.compareUnsigned(event.serial, lastSerial) <= 0) return;
            endpoint = registeredCapability;
            expectedTarget = attemptTarget;
        }
        try {
            DesktopRootProtocol.fact(endpoint, event.kind, event.incarnation, event.serial, event.keySnapshot);
        } catch (RemoteException | RuntimeException | LinkageError failure) {
            Log.e(TAG, "desktop root fact delivery failed closed", failure);
            abandonObservation(event.incarnation, expectedTarget);
            return;
        }
        synchronized (LOCK) {
            if (singleton != this || state == State.CLOSING
                    || attemptTarget != expectedTarget
                    || registeredIncarnation != event.incarnation) return;
            if (haveSerial && Long.compareUnsigned(event.serial, lastSerial) <= 0) return;
            lastSerial = event.serial;
            haveSerial = true;
            if (event.kind == FACT_CLOSED) state = State.CLOSING;
        }
    }

    private void abandonObservation(long incarnation, long expectedTarget) {
        long handle = 0;
        synchronized (LOCK) {
            if (singleton != this || registeredIncarnation != incarnation
                    || attemptTarget != expectedTarget || expectedTarget == 0) return;
            handle = attemptTarget;
            attemptTarget = 0;
            state = admissionClosed ? State.CLOSING : State.IDLE;
        }
        if (handle != 0) releaseNative(handle);
    }

    private static void releaseOwnedTarget(DesktopRootClient client, long candidate) {
        if (client == null || candidate == 0) return;
        synchronized (LOCK) {
            if (client.attemptTarget != candidate) return;
            client.attemptTarget = 0;
        }
        releaseNative(candidate);
    }

    private static boolean postLocked(Work work) {
        if (admissionClosed) return false;
        PENDING.add(work);
        try {
            if (mainHandler == null || !mainHandler.postAtTime(
                    work, QUEUE_TOKEN, SystemClock.uptimeMillis())) {
                PENDING.remove(work);
                return false;
            }
            return true;
        } catch (RuntimeException | LinkageError failure) {
            PENDING.remove(work);
            Log.e(TAG, "main-looper work post failed", failure);
            return false;
        }
    }

    private static void runWork(Work work) {
        synchronized (LOCK) {
            if (!PENDING.remove(work)) return;
            ++running;
        }
        try {
            work.action.run();
        } finally {
            synchronized (LOCK) { --running; }
        }
    }

    private static void requireIdentity(long value, String name) {
        if (value == 0) throw new IllegalStateException(name + " is zero");
    }

    private static boolean validFact(int kind, long incarnation, long serial, boolean key) {
        if (incarnation == 0 || serial == 0) return false;
        if (kind == FACT_ACTIVATED) return key;
        if (kind == FACT_RESIGNED) return !key;
        return kind == FACT_CLOSED;
    }

    private static void releaseNative(long handle) {
        try {
            nativeBoundary().releaseTarget(handle);
        } catch (RuntimeException | LinkageError failure) {
            Log.e(TAG, "desktop root native release failed", failure);
        }
    }

    private static final class Event {
        final int kind;
        final long incarnation;
        final long serial;
        final boolean keySnapshot;

        Event(int eventKind, long rootIncarnation, long eventSerial, boolean key) {
            kind = eventKind;
            incarnation = rootIncarnation;
            serial = eventSerial;
            keySnapshot = key;
        }
    }

    private static final class Work implements Runnable {
        final Runnable action;

        Work(Runnable operation) { action = operation; }

        @Override public void run() { runWork(this); }
    }

    interface NativeBridge {
        long acquireTarget();
        long targetIncarnation(long target);
        boolean observe(long target, DesktopRootClient callback);
        void releaseTarget(long target);
        DesktopRootKeyDecisionNative.Boundary keyDecisions();
    }

    private static final NativeBridge REAL_NATIVE = new NativeBridge() {
        @Override public long acquireTarget() { return DesktopRootClient.acquireTarget(); }
        @Override public long targetIncarnation(long target) {
            return DesktopRootClient.targetIncarnation(target);
        }
        @Override public boolean observe(long target, DesktopRootClient callback) {
            return DesktopRootClient.observe(target, callback);
        }
        @Override public void releaseTarget(long target) { DesktopRootClient.releaseTarget(target); }
        @Override public DesktopRootKeyDecisionNative.Boundary keyDecisions() {
            return DesktopRootKeyDecisionNative.INSTANCE;
        }
    };
    private static NativeBridge nativeBridge = REAL_NATIVE;

    private static NativeBridge nativeBoundary() {
        synchronized (LOCK) { return nativeBridge; }
    }
}
