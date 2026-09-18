package dev.darwinart.runtime.wm;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;

/** Synchronized WMS-owned focus selection and retained publication batches. */
public final class WindowFocusRegistry {
    public static final int FLAG_NOT_FOCUSABLE = 0x00000008;

    public enum WindowRole { APPLICATION, ATTACHED }
    public enum PublicationKind { GEOMETRY, FOCUS }

    /** Per-record result retained until an entire publication batch settles. */
    public enum Disposition { ACCEPTED, ABANDONED_TERMINAL }

    /** Separate identity binding for a genuine host-root activation. */
    public interface RootActivationBinding {
        Object rootToken();
        Object incarnation();
        int pid();
        int displayId();
    }

    /** Immutable activation identity; external mutable bindings are snapshotted on entry. */
    public static final class Activation implements RootActivationBinding {
        private final Object rootToken;
        private final Object incarnation;
        private final int pid;
        private final int displayId;

        public Activation(Object rootToken, Object incarnation, int pid, int displayId) {
            if (rootToken == null || incarnation == null || pid <= 0 || displayId < 0)
                throw new IllegalArgumentException("invalid root activation");
            this.rootToken = rootToken;
            this.incarnation = incarnation;
            this.pid = pid;
            this.displayId = displayId;
        }
        @Override public Object rootToken() { return rootToken; }
        @Override public Object incarnation() { return incarnation; }
        @Override public int pid() { return pid; }
        @Override public int displayId() { return displayId; }
    }

    /** Immutable WMS window identity and state snapshot. */
    public static final class WindowSpec {
        public final Object windowToken;
        public final Object channelIncarnation;
        public final int pid;
        public final Object rootToken;
        public final Object rootIncarnation;
        public final int displayId;
        public final Object parentToken;
        public final WindowRole role;
        public final int flags;
        public final boolean visible;
        public final int left;
        public final int top;
        public final int right;
        public final int bottom;
        /** Larger Android order is newer and wins; ties use insertion order. */
        public final long order;

        public WindowSpec(Object windowToken, Object channelIncarnation, int pid,
                Object rootToken, Object rootIncarnation, int displayId,
                Object parentToken, WindowRole role, int flags, boolean visible,
                int left, int top, int right, int bottom, long order) {
            // A snapshot may arrive before WMS has authenticated its desktop
            // root.  Such a snapshot is deliberately unbound: both root
            // identities are absent and neither one may be supplied alone.
            boolean rootTokenUnbound = rootToken == null;
            boolean rootIncarnationUnbound = rootIncarnation == null;
            if (windowToken == null || channelIncarnation == null || pid <= 0
                    || rootTokenUnbound != rootIncarnationUnbound || displayId < 0
                    || role == null || order < 0)
                throw new IllegalArgumentException("invalid window identity/state");
            if (role == WindowRole.APPLICATION && parentToken != null)
                throw new IllegalArgumentException("application window has parent");
            if (role == WindowRole.ATTACHED && parentToken == null)
                throw new IllegalArgumentException("attached window needs parent");
            this.windowToken = windowToken;
            this.channelIncarnation = channelIncarnation;
            this.pid = pid;
            this.rootToken = rootToken;
            this.rootIncarnation = rootIncarnation;
            this.displayId = displayId;
            this.parentToken = parentToken;
            this.role = role;
            this.flags = flags;
            this.visible = visible;
            this.left = left;
            this.top = top;
            this.right = right;
            this.bottom = bottom;
            this.order = order;
        }
    }

    /** Immutable publication record. Geometry has epoch 0; focus has a positive epoch. */
    public static final class Publication {
        public final PublicationKind kind;
        public final Object windowToken;
        public final Object channelIncarnation;
        public final int pid;
        public final Object rootToken;
        public final Object rootIncarnation;
        public final int displayId;
        public final Object parentToken;
        public final WindowRole role;
        public final int flags;
        public final long order;
        public final int left;
        public final int top;
        public final int right;
        public final int bottom;
        public final boolean visible;
        public final long epoch;
        public final boolean focused;

        private Publication(PublicationKind kind, WindowSpec window, long epoch,
                boolean focused, boolean visible, int left, int top, int right, int bottom) {
            this.kind = kind;
            this.windowToken = window.windowToken;
            this.channelIncarnation = window.channelIncarnation;
            this.pid = window.pid;
            this.rootToken = window.rootToken;
            this.rootIncarnation = window.rootIncarnation;
            this.displayId = window.displayId;
            this.parentToken = window.parentToken;
            this.role = window.role;
            this.flags = window.flags;
            this.order = window.order;
            this.left = left;
            this.top = top;
            this.right = right;
            this.bottom = bottom;
            this.visible = visible;
            this.epoch = epoch;
            this.focused = focused;
        }
        static Publication geometry(WindowSpec window) {
            return new Publication(PublicationKind.GEOMETRY, window, 0, false,
                    window.visible, window.left, window.top, window.right, window.bottom);
        }
        static Publication hidden(WindowSpec window) {
            return new Publication(PublicationKind.GEOMETRY, window, 0, false,
                    false, 0, 0, 0, 0);
        }
        static Publication focus(WindowSpec window, long epoch, boolean focused) {
            return new Publication(PublicationKind.FOCUS, window, epoch, focused,
                    window.visible, window.left, window.top, window.right, window.bottom);
        }
    }

    /** Immutable FIFO unit retained until the single publication owner acknowledges it. */
    public static final class PublicationBatch {
        public final long sequence;
        public final List<Publication> publications;
        private PublicationBatch(long sequence, List<Publication> values) {
            this.sequence = sequence;
            publications = Collections.unmodifiableList(new ArrayList<>(values));
        }
    }

    /** Cached owner decision used to replay current focus to a replacement receiver. */
    public static final class FocusSnapshot {
        public final WindowSpec window;
        public final long epoch;
        private FocusSnapshot(WindowSpec value, long transitionEpoch) {
            window = value;
            epoch = transitionEpoch;
        }
    }

    /** Authoritative decision, including the epoch of a no-winner revocation. */
    public static final class DecisionSnapshot {
        public final WindowSpec window;
        public final long epoch;
        private DecisionSnapshot(WindowSpec value, long transitionEpoch) {
            window = value;
            epoch = transitionEpoch;
        }
    }

    private static final class StoredWindow {
        final WindowSpec spec;
        final long arrival;
        StoredWindow(WindowSpec value, long sequence) { spec = value; arrival = sequence; }
    }
    private static final class FocusedWindow {
        final StoredWindow window;
        final long epoch;
        FocusedWindow(StoredWindow value, long transitionEpoch) {
            window = value;
            epoch = transitionEpoch;
        }
    }

    private IdentityHashMap<Object, StoredWindow> windows = new IdentityHashMap<>();
    private Map<Integer, Activation> activeRoots = new HashMap<>();
    private Map<Integer, FocusedWindow> focused = new HashMap<>();
    private Map<Integer, Long> decisionEpochs = new HashMap<>();
    private List<PublicationBatch> pending = new ArrayList<>();
    private long arrivalSequence;
    private long epoch;
    private long batchSequence;

    public WindowFocusRegistry() {}

    /** Publishes a new genuine host-root activation, replacing that display's old one. */
    public synchronized void activateRoot(Activation activation) {
        if (activation == null) throw new IllegalArgumentException("invalid root activation");
        Activation old = activeRoots.get(activation.displayId());
        if (sameActivation(old, activation)) return;
        Map<Integer, Activation> nextRoots = new HashMap<>(activeRoots);
        nextRoots.put(activation.displayId(), activation);
        applyPrepared(nextRoots, new IdentityHashMap<>(windows), activation.displayId(), null,
                false, null, null);
    }

    /** Resigns only the exact activation; a late old resign cannot clear a newer one. */
    public synchronized void resignRoot(Activation activation) {
        if (activation == null) throw new IllegalArgumentException("invalid root activation");
        Activation current = activeRoots.get(activation.displayId());
        if (!sameActivation(current, activation)) return;
        Map<Integer, Activation> nextRoots = new HashMap<>(activeRoots);
        nextRoots.remove(activation.displayId());
        applyPrepared(nextRoots, new IdentityHashMap<>(windows), activation.displayId(), null,
                false, null, null);
    }

    /** Adds or updates a window; identical state is a strict no-op. */
    public synchronized void upsert(WindowSpec spec) {
        if (spec == null) throw new IllegalArgumentException("window is null");
        StoredWindow old = windows.get(spec.windowToken);
        if (old != null && sameSpec(old.spec, spec)) return;
        long arrival = old != null && old.spec.channelIncarnation == spec.channelIncarnation
                ? old.arrival : nextValue(arrivalSequence, "window order exhausted");
        StoredWindow replacement = new StoredWindow(spec, arrival);
        IdentityHashMap<Object, StoredWindow> nextWindows = new IdentityHashMap<>(windows);
        nextWindows.put(spec.windowToken, replacement);
        List<Publication> publications = new ArrayList<>();
        if (old != null && (old.spec.channelIncarnation != spec.channelIncarnation
                || old.spec.displayId != spec.displayId)) {
            // Invalidate the predecessor resource/geometry before publishing
            // the successor, even when the logical window token is reused.
            publications.add(Publication.hidden(old.spec));
        }
        publications.add(Publication.geometry(spec));
        int display = old != null && old.spec.displayId != spec.displayId
                ? Integer.MIN_VALUE : spec.displayId;
        applyPrepared(new HashMap<>(activeRoots), nextWindows, display, publications, true,
                old == null ? null : old.spec.displayId, spec.displayId);
    }

    /** Removes only a window's exact channel incarnation and publishes a hide geometry. */
    public synchronized void remove(Object windowToken, Object channelIncarnation) {
        if (windowToken == null || channelIncarnation == null) return;
        StoredWindow current = windows.get(windowToken);
        if (current == null || current.spec.channelIncarnation != channelIncarnation) return;
        IdentityHashMap<Object, StoredWindow> nextWindows = new IdentityHashMap<>(windows);
        nextWindows.remove(windowToken);
        List<Publication> publications = new ArrayList<>();
        publications.add(Publication.hidden(current.spec));
        applyPrepared(new HashMap<>(activeRoots), nextWindows, current.spec.displayId,
                publications, true, null, null);
    }

    public synchronized WindowSpec focusedWindow(int displayId) {
        FocusedWindow selected = focused.get(displayId);
        return selected == null ? null : selected.window.spec;
    }

    /** Returns the cached winner and owner epoch for receiver replacement/replay. */
    public synchronized FocusSnapshot focusSnapshot(int displayId) {
        FocusedWindow selected = focused.get(displayId);
        return selected == null ? null : new FocusSnapshot(selected.window.spec, selected.epoch);
    }

    /** Snapshot under WMS ownership; epoch zero means no transition yet, not a grant. */
    public synchronized DecisionSnapshot decisionSnapshot(int displayId) {
        FocusedWindow selected = focused.get(displayId);
        Long lastEpoch = decisionEpochs.get(displayId);
        return new DecisionSnapshot(selected == null ? null : selected.window.spec,
                lastEpoch == null ? 0L : lastEpoch);
    }

    public synchronized RootActivationBinding activeRoot(int displayId) {
        return activeRoots.get(displayId);
    }

    /**
     * Returns whether the exact channel incarnation is still owned by a
     * current window or retained by any publication batch.  Publication
     * records are checked through identity, since channel values are opaque
     * resource incarnations rather than value objects.
     */
    public synchronized boolean referencesChannel(Object exactChannel) {
        if (exactChannel == null) return false;
        for (StoredWindow window : windows.values()) {
            if (window.spec.channelIncarnation == exactChannel) return true;
        }
        for (PublicationBatch batch : pending) {
            for (Publication publication : batch.publications) {
                if (publication.channelIncarnation == exactChannel) return true;
            }
        }
        return false;
    }

    /** Returns the exact FIFO head; the owner drains it outside registry locks. */
    public synchronized PublicationBatch pendingBatch() {
        return pending.isEmpty() ? null : pending.get(0);
    }

    /** Acknowledges only the exact current FIFO head. A stale batch cannot skip the suffix. */
    public synchronized boolean ackAccepted(PublicationBatch batch) {
        if (batch == null || pending.isEmpty() || pending.get(0) != batch) return false;
        pending.remove(0);
        return true;
    }

    /**
     * Settles the exact FIFO head with caller-owned disposition values. The
     * values are copied before validation/consumption, so a later mutation of
     * the caller's list cannot alter the recorded outcome.
     */
    public synchronized boolean ackSettled(PublicationBatch batch,
            List<Disposition> dispositions) {
        if (dispositions == null) return false;
        Disposition[] snapshot;
        try {
            snapshot = dispositions.toArray(new Disposition[dispositions.size()]);
        } catch (RuntimeException failure) {
            return false;
        }
        return ackSettledPrepared(batch, snapshot);
    }

    /** Internal allocation-free acknowledgement for a delivery owner. */
    synchronized boolean ackSettledPrepared(PublicationBatch batch,
            Disposition[] dispositions) {
        if (batch == null || dispositions == null || pending.isEmpty()
                || pending.get(0) != batch || dispositions.length != batch.publications.size())
            return false;
        for (Disposition disposition : dispositions)
            if (disposition == null) return false;
        pending.remove(0);
        return true;
    }

    private void applyPrepared(Map<Integer, Activation> nextRoots,
            IdentityHashMap<Object, StoredWindow> nextWindows, int display,
            List<Publication> initial, boolean commitWindows, Integer oldDisplay,
            Integer destinationDisplayHint) {
        Map<Integer, FocusedWindow> nextFocused = new HashMap<>(focused);
        Map<Integer, Long> nextDecisionEpochs = new HashMap<>(decisionEpochs);
        long[] nextEpoch = new long[] {epoch};
        long[] sharedEpoch = display == Integer.MIN_VALUE ? new long[] {0} : null;
        List<Publication> publications = initial == null
                ? new ArrayList<Publication>() : new ArrayList<>(initial);
        if (display == Integer.MIN_VALUE) {
            if (oldDisplay != null)
                appendTransition(oldDisplay, nextRoots, nextWindows, nextFocused, publications,
                        nextEpoch, sharedEpoch, nextDecisionEpochs);
            int destination = destinationDisplayHint == null
                    ? destinationDisplay(nextWindows, oldDisplay) : destinationDisplayHint;
            if (destination >= 0)
                appendTransition(destination, nextRoots, nextWindows, nextFocused, publications,
                        nextEpoch, sharedEpoch, nextDecisionEpochs);
        } else {
            appendTransition(display, nextRoots, nextWindows, nextFocused, publications, nextEpoch,
                    null, nextDecisionEpochs);
        }
        if (publications.isEmpty()) {
            commitMaps(nextRoots, nextWindows, nextFocused, commitWindows);
            decisionEpochs = nextDecisionEpochs;
            return;
        }
        long sequence = nextValue(batchSequence, "publication sequence exhausted");
        PublicationBatch batch = new PublicationBatch(sequence, publications);
        List<PublicationBatch> nextPending = new ArrayList<>(pending);
        nextPending.add(batch);
        long preparedArrival = commitWindows ? inferArrivalSequence(nextWindows) : arrivalSequence;
        // Every allocation capable of failing is complete before this state commit.
        commitMaps(nextRoots, nextWindows, nextFocused, commitWindows);
        decisionEpochs = nextDecisionEpochs;
        arrivalSequence = preparedArrival;
        epoch = nextEpoch[0];
        batchSequence = sequence;
        pending = nextPending;
    }

    private int destinationDisplay(IdentityHashMap<Object, StoredWindow> values,
            Integer oldDisplay) {
        for (StoredWindow value : values.values()) {
            if (oldDisplay == null || value.spec.displayId != oldDisplay)
                return value.spec.displayId;
        }
        return -1;
    }

    private void appendTransition(int display, Map<Integer, Activation> roots,
            IdentityHashMap<Object, StoredWindow> values, Map<Integer, FocusedWindow> selections,
            List<Publication> publications, long[] nextEpoch, long[] sharedEpoch,
            Map<Integer, Long> nextDecisionEpochs) {
        StoredWindow next = select(display, roots, values);
        FocusedWindow oldFocused = selections.get(display);
        StoredWindow old = oldFocused == null ? null : oldFocused.window;
        if (sameFocusIdentity(old, next)) {
            if (oldFocused != null && oldFocused.window != next)
                selections.put(display, new FocusedWindow(next, oldFocused.epoch));
            return;
        }
        long transitionEpoch;
        if (sharedEpoch != null) {
            if (sharedEpoch[0] == 0) {
                nextEpoch[0] = nextValue(nextEpoch[0], "focus epoch exhausted");
                sharedEpoch[0] = nextEpoch[0];
            }
            transitionEpoch = sharedEpoch[0];
        } else {
            nextEpoch[0] = nextValue(nextEpoch[0], "focus epoch exhausted");
            transitionEpoch = nextEpoch[0];
        }
        if (next == null) selections.remove(display);
        else selections.put(display, new FocusedWindow(next, transitionEpoch));
        nextDecisionEpochs.put(display, transitionEpoch);
        if (old != null) publications.add(Publication.focus(old.spec, transitionEpoch, false));
        if (next != null) publications.add(Publication.focus(next.spec, transitionEpoch, true));
    }

    private StoredWindow select(int display, Map<Integer, Activation> roots,
            IdentityHashMap<Object, StoredWindow> values) {
        Activation active = roots.get(display);
        if (active == null) return null;
        StoredWindow selected = null;
        for (StoredWindow candidate : values.values()) {
            if (!eligible(candidate.spec, active, values)) continue;
            if (selected == null || newer(candidate, selected)) selected = candidate;
        }
        return selected;
    }

    private boolean eligible(WindowSpec spec, Activation active,
            IdentityHashMap<Object, StoredWindow> values) {
        // Unbound snapshots can publish geometry while binding is in flight,
        // but they are never eligible for focus, even if a same-pid root is
        // active on the display.  Do not infer or synthesize root activation.
        if (spec.rootToken == null || spec.rootIncarnation == null
                || spec.displayId != active.displayId() || spec.pid != active.pid()
                || spec.rootToken != active.rootToken()
                || spec.rootIncarnation != active.incarnation() || !spec.visible
                || spec.right <= spec.left || spec.bottom <= spec.top
                || (spec.flags & FLAG_NOT_FOCUSABLE) != 0) return false;
        if (spec.role == WindowRole.APPLICATION) return true;
        StoredWindow parent = values.get(spec.parentToken);
        return parent != null && parent.spec.role == WindowRole.APPLICATION
                && parent.spec.pid == spec.pid && parent.spec.displayId == spec.displayId
                && parent.spec.rootToken == spec.rootToken
                && parent.spec.rootIncarnation == spec.rootIncarnation
                && parent.spec.visible && parent.spec.right > parent.spec.left
                && parent.spec.bottom > parent.spec.top;
    }

    private static boolean newer(StoredWindow left, StoredWindow right) {
        return left.spec.order > right.spec.order
                || (left.spec.order == right.spec.order && left.arrival > right.arrival);
    }

    private static boolean sameFocusIdentity(StoredWindow left, StoredWindow right) {
        if (left == null || right == null) return left == right;
        WindowSpec a = left.spec;
        WindowSpec b = right.spec;
        return a.windowToken == b.windowToken && a.channelIncarnation == b.channelIncarnation
                && a.pid == b.pid && a.displayId == b.displayId
                && a.rootToken == b.rootToken && a.rootIncarnation == b.rootIncarnation;
    }

    private static boolean sameSpec(WindowSpec a, WindowSpec b) {
        return a.windowToken == b.windowToken && a.channelIncarnation == b.channelIncarnation
                && a.pid == b.pid && a.rootToken == b.rootToken
                && a.rootIncarnation == b.rootIncarnation && a.displayId == b.displayId
                && a.parentToken == b.parentToken && a.role == b.role && a.flags == b.flags
                && a.visible == b.visible && a.left == b.left && a.top == b.top
                && a.right == b.right && a.bottom == b.bottom && a.order == b.order;
    }

    private void commitMaps(Map<Integer, Activation> roots,
            IdentityHashMap<Object, StoredWindow> values, Map<Integer, FocusedWindow> selections,
            boolean commitWindows) {
        activeRoots = roots;
        focused = selections;
        if (commitWindows) windows = values;
    }

    private long inferArrivalSequence(IdentityHashMap<Object, StoredWindow> values) {
        long result = arrivalSequence;
        for (StoredWindow value : values.values()) if (value.arrival > result) result = value.arrival;
        return result;
    }

    private static long nextValue(long value, String message) {
        if (value == Long.MAX_VALUE) throw new IllegalStateException(message);
        return value + 1;
    }

    private static boolean sameActivation(Activation a, Activation b) {
        return a != null && b != null && a.rootToken() == b.rootToken()
                && a.incarnation() == b.incarnation() && a.pid() == b.pid()
                && a.displayId() == b.displayId();
    }
}
