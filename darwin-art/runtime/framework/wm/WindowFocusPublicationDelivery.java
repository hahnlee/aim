package dev.darwinart.runtime.wm;

import java.util.List;

/** WMS-owned exact delivery of retained immutable focus publication batches. */
public final class WindowFocusPublicationDelivery {
    public interface Transport {
        Result publish(WindowFocusRegistry.Publication publication);

        /**
         * True only when the exact original endpoint TX is terminal and all
         * admitted work is quiescent. This is not release/dispose and does
         * not prove that a remote receiver never observed the publication.
         */
        default boolean terminateAndQuiesce(WindowFocusRegistry.Publication exactOriginal) {
            return false;
        }
    }

    /** ACCEPTED means locally retained by transport, not receiver notification or TX drain. */
    public enum Result { ACCEPTED, BACKPRESSURED, TERMINAL }
    public enum DrainResult { DRAINED, BLOCKED, TERMINAL, BUDGET_EXHAUSTED, COALESCED }
    public enum QuarantineReason { TERMINAL, NULL_RESULT, THROWABLE, UNKNOWN_RESULT }
    public enum SettlementResult { SETTLED, NOT_QUIESCENT, COALESCED, INVALID }

    /** Opaque owner/batch/index/publication identity for one external attempt. */
    public static final class Attempt {
        private final WindowFocusPublicationDelivery owner;
        private final WindowFocusRegistry.PublicationBatch batch;
        private final int index;
        private final WindowFocusRegistry.Publication publication;

        private Attempt(WindowFocusPublicationDelivery owner,
                WindowFocusRegistry.PublicationBatch batch, int index,
                WindowFocusRegistry.Publication publication) {
            this.owner = owner;
            this.batch = batch;
            this.index = index;
            this.publication = publication;
        }
    }

    private final WindowFocusRegistry registry;
    private final Transport transport;
    private final Object stateLock = new Object();
    private WindowFocusRegistry.PublicationBatch pendingBatch;
    private WindowFocusRegistry.Disposition[] dispositions;
    private int cursor;
    private Attempt pendingAttempt;
    private QuarantineReason quarantineReason;
    private boolean attemptInFlight;
    private boolean draining;
    private boolean lostHead;

    public WindowFocusPublicationDelivery(WindowFocusRegistry registry, Transport transport) {
        if (registry == null || transport == null)
            throw new IllegalArgumentException("registry/transport is null");
        this.registry = registry;
        this.transport = transport;
    }

    /**
     * Returns the exact quarantined token, never one whose publish callback is
     * in flight. A termination callback may inspect this same token; a nested
     * settlement then coalesces through the draining guard.
     */
    public Attempt pendingTerminalAttempt() {
        synchronized (stateLock) {
            return attemptInFlight || quarantineReason == null ? null : pendingAttempt;
        }
    }

    public QuarantineReason quarantineReason() {
        synchronized (stateLock) { return quarantineReason; }
    }

    /**
     * Delivers at most {@code budget} records; nested/concurrent calls coalesce.
     * Backpressure needs a later progress turn from the transport owner.
     * Terminal/null/thrown outcomes remain quarantined, never blindly replayed.
     * Transport callbacks run outside both owner and registry monitors.
     */
    public DrainResult drain(int budget) {
        if (budget <= 0) throw new IllegalArgumentException("budget must be positive");
        synchronized (stateLock) {
            if (draining) return DrainResult.COALESCED;
            draining = true;
        }
        try {
            return drainExclusive(budget);
        } finally {
            synchronized (stateLock) { draining = false; }
        }
    }

    /** Settles only the exact quarantined attempt; uncertain sends are never replayed. */
    public SettlementResult settleTerminal(Attempt exact) {
        synchronized (stateLock) {
            if (draining) return SettlementResult.COALESCED;
            draining = true;
        }
        try {
            return settleTerminalExclusive(exact);
        } finally {
            synchronized (stateLock) { draining = false; }
        }
    }

    private SettlementResult settleTerminalExclusive(Attempt exact) {
        if (!validateSettlementAttempt(exact)) return SettlementResult.INVALID;
        final boolean terminated;
        try {
            // External transport callback runs without owner or registry locks.
            terminated = transport.terminateAndQuiesce(exact.publication);
        } catch (Throwable failure) {
            validateAfterExternalCallback(exact);
            throw failure;
        }
        validateAfterExternalCallback(exact);
        if (!terminated) return SettlementResult.NOT_QUIESCENT;
        if (!advanceAbandoned(exact)) return SettlementResult.INVALID;
        return SettlementResult.SETTLED;
    }

    private DrainResult drainExclusive(int budget) {
        int published = 0;
        while (published < budget) {
            WindowFocusRegistry.PublicationBatch batch = ownedBatch();
            if (batch == null) {
                batch = registry.pendingBatch();
                if (batch == null) return DrainResult.DRAINED;
                adoptBatch(batch);
            }
            if (lostHead || registry.pendingBatch() != batch) throw lostBatch();
            int index;
            synchronized (stateLock) {
                if (pendingBatch != batch || lostHead) throw lostBatch();
                if (quarantineReason != null) return DrainResult.TERMINAL;
                index = cursor;
                if (pendingAttempt != null || attemptInFlight) throw lostBatch();
            }
            List<WindowFocusRegistry.Publication> publications = batch.publications;
            if (index < 0 || index > publications.size()) throw lostBatch();
            if (index == publications.size()) {
                acknowledge(batch);
                continue;
            }

            // Allocate the opaque token before invoking any external code.
            Attempt attempt = new Attempt(this, batch, index, publications.get(index));
            synchronized (stateLock) {
                if (pendingBatch != batch || cursor != index || quarantineReason != null
                        || pendingAttempt != null) throw lostBatch();
                pendingAttempt = attempt;
                attemptInFlight = true;
            }

            final Result result;
            try {
                validateBeforeExternalCallback(attempt);
                result = transport.publish(attempt.publication);
            } catch (Throwable failure) {
                latchThrowable(attempt);
                throw failure;
            }
            validateAfterExternalCallback(attempt);
            if (result == null) {
                latch(attempt, QuarantineReason.NULL_RESULT);
                throw new IllegalStateException("null publication result");
            }
            if (result == Result.ACCEPTED) {
                markAccepted(attempt);
                ++published;
                if (index + 1 == publications.size()) acknowledge(batch);
            } else if (result == Result.BACKPRESSURED) {
                clearInFlight(attempt);
                return DrainResult.BLOCKED;
            } else if (result == Result.TERMINAL) {
                latch(attempt, QuarantineReason.TERMINAL);
                return DrainResult.TERMINAL;
            } else {
                latch(attempt, QuarantineReason.UNKNOWN_RESULT);
                throw new IllegalStateException("unknown publication result");
            }
        }

        WindowFocusRegistry.PublicationBatch batch = ownedBatch();
        if (batch != null) {
            if (lostHead || registry.pendingBatch() != batch) throw lostBatch();
            synchronized (stateLock) {
                if (quarantineReason != null) return DrainResult.TERMINAL;
                if (pendingBatch != batch || cursor < 0 || cursor > batch.publications.size())
                    throw lostBatch();
                if (cursor < batch.publications.size()) return DrainResult.BUDGET_EXHAUSTED;
            }
        }
        return registry.pendingBatch() == null
                ? DrainResult.DRAINED : DrainResult.BUDGET_EXHAUSTED;
    }

    private void adoptBatch(WindowFocusRegistry.PublicationBatch batch) {
        // All outcome storage is allocated before the first external publish.
        WindowFocusRegistry.Disposition[] prepared =
                new WindowFocusRegistry.Disposition[batch.publications.size()];
        if (registry.pendingBatch() != batch) throw lostBatch();
        synchronized (stateLock) {
            if (pendingBatch == null) {
                pendingBatch = batch;
                dispositions = prepared;
                cursor = 0;
            } else if (pendingBatch != batch) {
                throw lostBatch();
            }
        }
    }

    private void acknowledge(WindowFocusRegistry.PublicationBatch batch) {
        WindowFocusRegistry.Disposition[] values;
        synchronized (stateLock) {
            if (pendingBatch != batch || cursor != batch.publications.size()
                    || quarantineReason != null || attemptInFlight) throw lostBatch();
            values = dispositions;
        }
        if (!registry.ackSettledPrepared(batch, values)) {
            markLostHead();
            throw lostBatch();
        }
        synchronized (stateLock) {
            if (pendingBatch != batch || cursor != batch.publications.size()) throw lostBatch();
            pendingBatch = null;
            dispositions = null;
            pendingAttempt = null;
            quarantineReason = null;
            cursor = 0;
        }
    }

    private boolean advanceAbandoned(Attempt attempt) {
        WindowFocusRegistry.PublicationBatch batch = attempt.batch;
        int index = attempt.index;
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt) || attemptInFlight) return false;
            dispositions[index] = WindowFocusRegistry.Disposition.ABANDONED_TERMINAL;
            cursor = index + 1;
            pendingAttempt = null;
            quarantineReason = null;
        }
        if (cursor == batch.publications.size()) acknowledge(batch);
        return true;
    }

    private void markAccepted(Attempt attempt) {
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt) || !attemptInFlight) throw lostBatch();
            dispositions[attempt.index] = WindowFocusRegistry.Disposition.ACCEPTED;
            cursor = attempt.index + 1;
            pendingAttempt = null;
            attemptInFlight = false;
        }
    }

    private void clearInFlight(Attempt attempt) {
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt) || !attemptInFlight) throw lostBatch();
            pendingAttempt = null;
            attemptInFlight = false;
        }
    }

    private void latchThrowable(Attempt attempt) {
        validateAfterExternalCallback(attempt);
        latch(attempt, QuarantineReason.THROWABLE);
    }

    private void latch(Attempt attempt, QuarantineReason reason) {
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt) || !attemptInFlight) throw lostBatch();
            quarantineReason = reason;
            attemptInFlight = false;
        }
    }

    private boolean validateSettlementAttempt(Attempt attempt) {
        if (attempt == null) return false;
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt) || attemptInFlight || quarantineReason == null)
                return false;
        }
        return registry.pendingBatch() == attempt.batch;
    }

    private void validateBeforeExternalCallback(Attempt attempt) {
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt) || !attemptInFlight) throw lostBatch();
        }
        if (registry.pendingBatch() != attempt.batch) {
            markLostHead();
            throw lostBatch();
        }
    }

    private void validateAfterExternalCallback(Attempt attempt) {
        synchronized (stateLock) {
            if (!validAttemptLocked(attempt)) {
                markLostHead();
                throw lostBatch();
            }
        }
        if (registry.pendingBatch() != attempt.batch) {
            markLostHead();
            throw lostBatch();
        }
    }

    private boolean validAttemptLocked(Attempt attempt) {
        return attempt.owner == this && pendingBatch == attempt.batch
                && pendingAttempt == attempt && cursor == attempt.index
                && attempt.index >= 0 && attempt.index < attempt.batch.publications.size()
                && attempt.batch.publications.get(attempt.index) == attempt.publication
                && dispositions != null && dispositions[attempt.index] == null;
    }

    private void markLostHead() {
        synchronized (stateLock) { lostHead = true; attemptInFlight = false; }
    }

    private WindowFocusRegistry.PublicationBatch ownedBatch() {
        synchronized (stateLock) { return pendingBatch; }
    }

    private static IllegalStateException lostBatch() {
        return new IllegalStateException("publication batch head changed or acknowledgement failed");
    }
}
