package dev.darwinart.runtime.am;

import android.os.IBinder;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;

/**
 * Owns the prepare/activate lifetime of one bound-service process launch.
 *
 * Registry publication and this ledger are state transitions; the profile
 * backend is always called after the launcher monitor is released.  Entries
 * are keyed by the daemon's exact PID and launch sequence, never by PID alone.
 */
final class BoundServiceProcessLauncher
        implements ProcessLaunchTransport {
    interface Backend {
        long[] prepare(String packageName, String processName, int uid, boolean isolated,
                long startSequence);
        /** Retires a prepared-but-never-activated cap, including OOM paths. */
        void abortPrepared(long handle);
        void activate(long handle);
        void cancel(long handle);
        void forget(long handle);
    }

    private enum State {
        PREPARED,
        ACTIVATING,
        ACTIVE,
        HANDOFF,
        DEAD,
        /** The registry could not prove this cap is ours to cancel. */
        AWAITING_DEATH,
        CANCEL_PENDING,
        FORGET_PENDING
    }

    /** Process disposition survives local capability cleanup and metadata removal. */
    private enum Disposition { LIVE, HANDOFF, DEAD, CANCELLED }

    private static final class Key {
        final int pid;
        final long sequence;

        Key(int processId, long startSequence) {
            pid = processId;
            sequence = startSequence;
        }

        @Override public int hashCode() {
            return 31 * pid + Long.hashCode(sequence);
        }

        @Override public boolean equals(Object other) {
            if (!(other instanceof Key)) return false;
            Key value = (Key) other;
            return pid == value.pid && sequence == value.sequence;
        }
    }

    private static final class Entry {
        final Key key;
        final long handle;
        State state = State.PREPARED;
        Disposition disposition = Disposition.LIVE;
        boolean activationStarted;
        boolean activationInFlight;
        boolean retirementInFlight;
        boolean retirementRequested;
        boolean cancelAcknowledged;
        boolean cancelInFlight;
        boolean forgetInFlight;

        Entry(Key launchKey, long preparedHandle) {
            key = launchKey;
            handle = preparedHandle;
        }
    }

    /** Opaque launcher-owned token; callers can observe only the exact identity tuple. */
    private static final class LaunchToken implements ProcessLaunchTransport.PreparedLaunch {
        final BoundServiceProcessLauncher owner;
        final Entry entry;

        LaunchToken(BoundServiceProcessLauncher launcher, Entry launchEntry) {
            owner = launcher;
            entry = launchEntry;
        }

        @Override public int pid() { return entry.key.pid; }
        @Override public long startSequence() { return entry.key.sequence; }
    }

    private static final class NativeBackend implements Backend {
        @Override public long[] prepare(String packageName, String processName, int uid,
                boolean isolated, long startSequence) {
            return nativePrepareBoundServiceProcess(
                    packageName, processName, uid, isolated, startSequence);
        }

        @Override public void activate(long handle) {
            nativeActivateBoundServiceProcess(handle);
        }

        @Override public void abortPrepared(long handle) {
            nativeAbortPreparedBoundServiceProcess(handle);
        }

        @Override public void cancel(long handle) {
            nativeCancelBoundServiceProcess(handle);
        }

        @Override public void forget(long handle) {
            nativeForgetBoundServiceProcess(handle);
        }
    }

    private final ApplicationProcessRegistry registry;
    private final Backend backend;
    private final Object monitor = new Object();
    private final HashMap<Key, Entry> entries = new HashMap<>();
    private final HashSet<Long> startedSequences = new HashSet<>();

    BoundServiceProcessLauncher(ApplicationProcessRegistry processRegistry) {
        this(processRegistry, new NativeBackend());
    }

    BoundServiceProcessLauncher(ApplicationProcessRegistry processRegistry, Backend launchBackend) {
        if (processRegistry == null || launchBackend == null) {
            throw new IllegalArgumentException("Missing bound-service launcher owner");
        }
        registry = processRegistry;
        backend = launchBackend;
    }

    @Override
    public ProcessLaunchTransport.PreparedLaunch prepare(String packageName, String processName,
            int uid, boolean isolated, long startSequence) {
        validate(packageName, processName, uid, startSequence);
        retryPendingCancellation();
        synchronized (monitor) {
            if (!startedSequences.add(startSequence)) {
                throw new IllegalStateException("Duplicate bound-service launch sequence");
            }
        }

        long[] prepared = backend.prepare(packageName, processName, uid, isolated, startSequence);
        int preparedPid = prepared != null && prepared.length == 2
                && prepared[0] > 0 && prepared[0] <= Integer.MAX_VALUE
                ? (int) prepared[0] : 0;
        long preparedHandle = prepared != null && prepared.length == 2 ? prepared[1] : 0;
        Entry entry = null;
        try {
            if (prepared == null || prepared.length != 2 || preparedPid == 0 || preparedHandle == 0) {
                throw new IllegalStateException("Invalid prepared bound-service process");
            }
            Key key = new Key(preparedPid, startSequence);
            entry = new Entry(key, preparedHandle);
            registry.reserveBoundServiceProcess(
                    key.pid, packageName, processName, uid, startSequence);
            synchronized (monitor) {
                entries.put(key, entry);
            }
            return new LaunchToken(this, entry);
        } catch (RuntimeException | Error failure) {
            Throwable cleanupFailure = abortUnactivated(preparedHandle, preparedPid,
                    startSequence, entry);
            if (cleanupFailure != null) failure.addSuppressed(cleanupFailure);
            throw failure;
        }
    }

    @Override
    public ProcessLaunchTransport.ActivationResult activate(
            ProcessLaunchTransport.PreparedLaunch prepared) {
        Entry entry = requireToken(prepared);
        synchronized (monitor) {
            if (entry.activationStarted) {
                throw new IllegalStateException("Bound-service launch activation already claimed");
            }
            entry.activationStarted = true;
            if (entry.disposition == Disposition.HANDOFF) {
                return ProcessLaunchTransport.ActivationResult.HANDOFF;
            }
            if (entry.disposition == Disposition.CANCELLED
                    || entry.disposition == Disposition.DEAD) {
                return ProcessLaunchTransport.ActivationResult.CANCELLED;
            }
            if (entries.get(entry.key) != entry) {
                throw new IllegalStateException("Stale bound-service launch token");
            }
            if (entry.retirementRequested) {
                throw new IllegalStateException("Bound-service retirement remains unresolved");
            }
            entry.state = State.ACTIVATING;
            entry.activationInFlight = true;
        }
        try {
            backend.activate(entry.handle);
        } catch (RuntimeException | Error failure) {
            return activationFailedResult(entry, failure);
        }
        List<Entry> forget = finishActivation(entry);
        reportCleanup(forgetOutsideMonitor(forget));
        reportCleanup(cancelOrForget(entry));
        synchronized (monitor) {
            if (entry.disposition == Disposition.HANDOFF) {
                return ProcessLaunchTransport.ActivationResult.HANDOFF;
            }
            if (entry.disposition == Disposition.CANCELLED
                    || entry.disposition == Disposition.DEAD) {
                return ProcessLaunchTransport.ActivationResult.CANCELLED;
            }
            if (entry.retirementRequested || entries.get(entry.key) != entry) {
                throw new IllegalStateException("Bound-service activation has unresolved retirement");
            }
            return ProcessLaunchTransport.ActivationResult.ACTIVE;
        }
    }

    @Override
    public void retireUnattached(ProcessLaunchTransport.PreparedLaunch prepared) {
        Entry entry = requireToken(prepared);
        boolean terminal;
        synchronized (monitor) {
            terminal = entry.disposition != Disposition.LIVE;
            if (entries.get(entry.key) != entry) {
                if (terminal) return;
                throw new IllegalStateException("Stale bound-service launch token");
            }
            if (!terminal && entry.retirementInFlight) {
                throw new IllegalStateException("Prepared launch is no longer unattached");
            }
            if (!terminal) {
                entry.retirementRequested = true;
                entry.retirementInFlight = true;
            }
        }
        if (terminal) {
            Throwable failure = retryTerminalCleanup(entry);
            if (failure != null) rethrow(failure);
            return;
        }
        ApplicationProcessRegistry.BoundServiceCancellation outcome;
        try {
            outcome = registry.claimBoundServiceCancellation(entry.key.pid, entry.key.sequence);
        } catch (RuntimeException | Error failure) {
            synchronized (monitor) {
                if (entries.get(entry.key) == entry) entry.retirementInFlight = false;
            }
            Throwable cleanup = retryTerminalCleanup(entry);
            if (cleanup != null && cleanup != failure) failure.addSuppressed(cleanup);
            throw failure;
        }
        if (outcome.kind == ApplicationProcessRegistry.CancellationKind.CANCELLED) {
            synchronized (monitor) {
                if (entries.get(entry.key) == entry) {
                    entry.state = State.CANCEL_PENDING;
                    entry.disposition = Disposition.CANCELLED;
                    entry.retirementInFlight = false;
                }
            }
            Throwable failure = cancelOrForget(entry);
            if (failure != null) rethrow(failure);
            return;
        }
        if (outcome.kind == ApplicationProcessRegistry.CancellationKind.ATTACHMENT_OWNED) {
            List<Entry> forget;
            synchronized (monitor) {
                if (entries.get(entry.key) != entry) return;
                entry.retirementInFlight = false;
                if (entry.disposition == Disposition.LIVE) {
                    entry.state = State.HANDOFF;
                    entry.disposition = Disposition.HANDOFF;
                }
                forget = remove(entry);
            }
            Throwable failure = forgetOutsideMonitor(forget);
            if (failure != null) rethrow(failure);
            return;
        }
        synchronized (monitor) {
            if (entries.get(entry.key) == entry) {
                entry.retirementInFlight = false;
                terminal = entry.disposition != Disposition.LIVE;
                if (!terminal) entry.state = State.AWAITING_DEATH;
            }
        }
        if (terminal) {
            Throwable failure = retryTerminalCleanup(entry);
            if (failure != null) rethrow(failure);
            return;
        }
        throw new IllegalStateException("Bound-service launch retirement lost exact registry ownership");
    }

    /** A terminal token may retry cleanup, but cannot reopen process cancellation. */
    private Throwable retryTerminalCleanup(Entry entry) {
        boolean cancel;
        List<Entry> forget;
        synchronized (monitor) {
            if (entries.get(entry.key) != entry || entry.disposition == Disposition.LIVE) {
                return null;
            }
            cancel = entry.state == State.CANCEL_PENDING;
            forget = cancel ? new ArrayList<>() : beginForget(entry);
        }
        return cancel ? cancelOrForget(entry) : forgetOutsideMonitor(forget);
    }

    /** Marks an exact registry attachment as handed off to the app process. */
    void onAttached(ApplicationProcessRegistry.AttachedApplication attached) {
        if (attached == null || attached.initialWork
                != ApplicationProcessRegistry.InitialWork.BOUND_SERVICE) return;
        List<Entry> forget;
        synchronized (monitor) {
            Entry entry = entries.get(new Key(attached.pid, attached.startSequence));
            if (entry == null || entry.state == State.CANCEL_PENDING
                    || entry.disposition == Disposition.DEAD
                    || entry.disposition == Disposition.CANCELLED) return;
            if (entry.state == State.FORGET_PENDING) return;
            entry.state = State.HANDOFF;
            entry.disposition = Disposition.HANDOFF;
            forget = entry.activationInFlight ? new ArrayList<>() : remove(entry);
        }
        reportCleanup(forgetOutsideMonitor(forget));
    }

    /** Marks an exact registry death snapshot; stale snapshots are ignored. */
    @Override public void onProcessGone(ApplicationProcessRegistry.AttachedApplication gone) {
        if (gone == null || gone.thread == null
                || gone.initialWork != ApplicationProcessRegistry.InitialWork.BOUND_SERVICE) return;
        List<Entry> forget;
        synchronized (monitor) {
            Entry entry = entries.get(new Key(gone.pid, gone.startSequence));
            if (entry == null || entry.disposition == Disposition.CANCELLED) return;
            entry.disposition = Disposition.DEAD;
            if (entry.state == State.FORGET_PENDING) return;
            entry.state = State.DEAD;
            forget = entry.activationInFlight ? new ArrayList<>() : remove(entry);
        }
        reportCleanup(forgetOutsideMonitor(forget));
    }

    /** Retries only cancellation/forget tails that previously failed. */
    void retryPendingCancellation() {
        List<Entry> pending;
        synchronized (monitor) {
            pending = new ArrayList<>();
            for (Entry entry : entries.values()) {
                if (entry.state == State.CANCEL_PENDING || entry.state == State.FORGET_PENDING) {
                    pending.add(entry);
                }
            }
        }
        for (Entry entry : pending) {
            State state;
            synchronized (monitor) {
                if (entries.get(entry.key) != entry) continue;
                state = entry.state;
            }
            if (state == State.CANCEL_PENDING) {
                reportCleanup(cancelOrForget(entry));
            } else if (state == State.FORGET_PENDING) {
                List<Entry> forget;
                synchronized (monitor) {
                    forget = beginForget(entry);
                }
                reportCleanup(forgetOutsideMonitor(forget));
            }
        }
    }

    private Entry requireToken(ProcessLaunchTransport.PreparedLaunch prepared) {
        if (!(prepared instanceof LaunchToken)
                || ((LaunchToken) prepared).owner != this) {
            throw new IllegalArgumentException("Foreign or invalid bound-service launch token");
        }
        Entry entry = ((LaunchToken) prepared).entry;
        if (entry == null) throw new IllegalArgumentException("Invalid bound-service launch token");
        return entry;
    }

    private ProcessLaunchTransport.ActivationResult activationFailedResult(
            Entry entry, Throwable failure) {
        ApplicationProcessRegistry.BoundServiceCancellation outcome;
        try {
            outcome = registry.claimBoundServiceCancellation(entry.key.pid, entry.key.sequence);
        } catch (RuntimeException | Error claimFailure) {
            markCancellationPending(entry);
            failure.addSuppressed(claimFailure);
            Throwable cleanupFailure = forgetOutsideMonitor(finishActivation(entry));
            if (cleanupFailure != null && cleanupFailure != failure) {
                failure.addSuppressed(cleanupFailure);
            }
            rethrow(failure);
            return ProcessLaunchTransport.ActivationResult.CANCELLED;
        }

        if (outcome.kind == ApplicationProcessRegistry.CancellationKind.ATTACHMENT_OWNED) {
            synchronized (monitor) {
                if (entries.get(entry.key) == entry && entry.disposition == Disposition.LIVE) {
                    entry.state = State.HANDOFF;
                    entry.disposition = Disposition.HANDOFF;
                }
            }
            List<Entry> forget = finishActivation(entry);
            Throwable cleanupFailure = forgetOutsideMonitor(forget);
            boolean handedOff;
            synchronized (monitor) {
                handedOff = entry.disposition == Disposition.HANDOFF;
            }
            if (handedOff) {
                reportCleanup(cleanupFailure);
                return ProcessLaunchTransport.ActivationResult.HANDOFF;
            }
            if (cleanupFailure != null && cleanupFailure != failure) {
                failure.addSuppressed(cleanupFailure);
            }
            rethrow(failure);
            return ProcessLaunchTransport.ActivationResult.CANCELLED;
        }
        if (outcome.kind == ApplicationProcessRegistry.CancellationKind.CANCELLED) {
            synchronized (monitor) {
                if (entries.get(entry.key) == entry) {
                    entry.activationInFlight = false;
                    entry.state = State.CANCEL_PENDING;
                    entry.disposition = Disposition.CANCELLED;
                    entry.retirementRequested = true;
                }
            }
            Throwable cleanupFailure = cancelOrForget(entry);
            if (cleanupFailure != null) failure.addSuppressed(cleanupFailure);
            rethrow(failure);
            return ProcessLaunchTransport.ActivationResult.CANCELLED;
        }

        // STALE_OR_GONE is not proof that this launch owns a cancellable
        // reservation.  Only an exact DEAD/HANDOFF ledger state permits
        // resource retirement; otherwise preserve the cap for retry.
        synchronized (monitor) {
            if (entries.get(entry.key) == entry
                    && entry.disposition != Disposition.LIVE) {
                entry.activationInFlight = false;
            } else {
                entry.state = State.AWAITING_DEATH;
                entry.activationInFlight = false;
            }
        }
        List<Entry> forget = finishActivation(entry);
        Throwable cleanupFailure = forgetOutsideMonitor(forget);
        if (cleanupFailure != null) failure.addSuppressed(cleanupFailure);
        rethrow(failure);
        return ProcessLaunchTransport.ActivationResult.CANCELLED;
    }

    private List<Entry> finishActivation(Entry entry) {
        synchronized (monitor) {
            if (entries.get(entry.key) != entry) return new ArrayList<>();
            entry.activationInFlight = false;
            if (entry.state == State.HANDOFF || entry.state == State.DEAD) {
                return remove(entry);
            }
            if (entry.state == State.ACTIVATING) entry.state = State.ACTIVE;
            return new ArrayList<>();
        }
    }

    private Throwable abortUnactivated(long handle, int pid, long sequence, Entry entry) {
        Throwable firstFailure = null;
        if (entry != null) {
            synchronized (monitor) {
                entries.remove(entry.key, entry);
            }
        }
        if (pid > 0) {
            try {
                // A stale result is harmless: the PID may already belong to a
                // different exact sequence, while a matching reservation is
                // atomically sealed against late attachment here.
                registry.claimBoundServiceCancellation(pid, sequence);
            } catch (RuntimeException | Error failure) {
                firstFailure = failure;
            }
        }
        if (handle != 0) {
            try {
                backend.abortPrepared(handle);
            } catch (RuntimeException | Error failure) {
                if (firstFailure == null) firstFailure = failure;
                else firstFailure.addSuppressed(failure);
            }
        }
        return firstFailure;
    }

    private Throwable cancelOrForget(Entry entry) {
        boolean acknowledged;
        synchronized (monitor) {
            if (entries.get(entry.key) != entry || entry.state != State.CANCEL_PENDING
                    || entry.retirementInFlight) {
                return null;
            }
            if (entry.cancelInFlight) return null;
            entry.cancelInFlight = true;
            acknowledged = entry.cancelAcknowledged;
        }
        if (!acknowledged) {
            try {
                backend.cancel(entry.handle);
            } catch (RuntimeException | Error failure) {
                synchronized (monitor) {
                    if (entries.get(entry.key) == entry) entry.cancelInFlight = false;
                }
                return failure;
            }
            synchronized (monitor) {
                if (entries.get(entry.key) != entry) return null;
                entry.cancelAcknowledged = true;
                entry.cancelInFlight = false;
            }
        } else {
            synchronized (monitor) {
                if (entries.get(entry.key) == entry) entry.cancelInFlight = false;
            }
        }
        List<Entry> forget;
        synchronized (monitor) {
            forget = beginForget(entry);
        }
        return forgetOutsideMonitor(forget);
    }

    private void markCancellationPending(Entry entry) {
        synchronized (monitor) {
            if (entries.get(entry.key) == entry) {
                entry.activationInFlight = false;
                if (entry.disposition == Disposition.LIVE) entry.state = State.AWAITING_DEATH;
            }
        }
    }

    private List<Entry> remove(Entry entry) {
        if (entries.get(entry.key) != entry) return new ArrayList<>();
        return beginForget(entry);
    }

    private List<Entry> beginForget(Entry entry) {
        if (entries.get(entry.key) != entry || entry.forgetInFlight
                || entry.activationInFlight || entry.cancelInFlight
                || entry.retirementInFlight) return new ArrayList<>();
        entry.state = State.FORGET_PENDING;
        entry.forgetInFlight = true;
        return java.util.Collections.singletonList(entry);
    }

    private Throwable forgetOutsideMonitor(List<Entry> forget) {
        Throwable firstFailure = null;
        for (Entry entry : forget) {
            try {
                backend.forget(entry.handle);
                synchronized (monitor) {
                    if (entries.get(entry.key) == entry
                            && entry.state == State.FORGET_PENDING) {
                        entries.remove(entry.key);
                        entry.forgetInFlight = false;
                    }
                }
            } catch (RuntimeException | Error failure) {
                synchronized (monitor) {
                    if (entries.get(entry.key) == entry
                            && entry.state == State.FORGET_PENDING) {
                        // The terminal callback has already happened, so it
                        // cannot be replayed to trigger another cleanup. Keep
                        // the exact cap explicitly retryable instead.
                        entry.state = State.FORGET_PENDING;
                        entry.forgetInFlight = false;
                    }
                }
                if (firstFailure == null) firstFailure = failure;
            }
        }
        return firstFailure;
    }

    private static void reportCleanup(Throwable failure) {
        if (failure == null) return;
        // The exact entry remains CANCEL_PENDING/FORGET_PENDING. A local
        // capability cleanup failure must not invalidate a real attachment or
        // block another process's launch; the next start retries the ledger.
        System.err.println("Bound-service launch cleanup remains owned: " + failure);
    }

    private static void validate(String packageName, String processName, int uid, long sequence) {
        if (packageName == null || packageName.isEmpty() || processName == null
                || processName.isEmpty() || uid < 0 || sequence <= 0) {
            throw new IllegalArgumentException("Invalid bound-service process identity");
        }
    }

    private static void rethrow(Throwable failure) {
        if (failure instanceof RuntimeException) throw (RuntimeException) failure;
        if (failure instanceof Error) throw (Error) failure;
        throw new AssertionError(failure);
    }

    private static native long[] nativePrepareBoundServiceProcess(
            String packageName, String processName, int uid, boolean isolated, long startSequence);
    private static native void nativeActivateBoundServiceProcess(long handle);
    private static native void nativeAbortPreparedBoundServiceProcess(long handle);
    private static native void nativeCancelBoundServiceProcess(long handle);
    private static native void nativeForgetBoundServiceProcess(long handle);
}
