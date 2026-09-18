package dev.darwinart.runtime.wm;

import java.util.ArrayList;
import java.util.List;

public final class WindowFocusRegistryTest {
    private static WindowFocusRegistry.WindowSpec app(Object window, Object channel,
            Object root, Object incarnation, int pid, boolean visible, int flags, long order,
            int right) {
        return new WindowFocusRegistry.WindowSpec(window, channel, pid, root, incarnation, 0,
                null, WindowFocusRegistry.WindowRole.APPLICATION, flags, visible,
                0, 0, right, 1280, order);
    }

    private static WindowFocusRegistry.WindowSpec unboundApp(Object window, Object channel,
            int pid, boolean visible, int flags, long order, int right) {
        return app(window, channel, null, null, pid, visible, flags, order, right);
    }

    private static WindowFocusRegistry.WindowSpec popup(Object window, Object channel,
            Object parent, Object root, Object incarnation, int pid, boolean visible,
            int flags, long order) {
        return new WindowFocusRegistry.WindowSpec(window, channel, pid, root, incarnation, 0,
                parent, WindowFocusRegistry.WindowRole.ATTACHED, flags, visible,
                10, 20, 200, 300, order);
    }

    private static WindowFocusRegistry.Activation activation(Object root, Object incarnation,
            int pid) {
        return new WindowFocusRegistry.Activation(root, incarnation, pid, 0);
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static List<WindowFocusRegistry.Publication> drain(
            WindowFocusRegistry registry) {
        List<WindowFocusRegistry.Publication> result = new ArrayList<>();
        WindowFocusRegistry.PublicationBatch batch;
        while ((batch = registry.pendingBatch()) != null) {
            result.addAll(batch.publications);
            check(registry.ackAccepted(batch), "exact batch acknowledgement");
        }
        return result;
    }

    private static void checkFocus(WindowFocusRegistry.Publication event, Object window,
            Object channel, long epoch, boolean focused) {
        check(event.kind == WindowFocusRegistry.PublicationKind.FOCUS, "focus publication");
        check(event.windowToken == window && event.channelIncarnation == channel,
                "exact focus identity");
        check(event.epoch == epoch && event.focused == focused, "focus state/epoch");
    }

    private static void testGeometryAndPopupRestoration() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object root = new Object(), incarnation = new Object();
        Object app = new Object(), appChannel = new Object();
        Object popup = new Object(), popupChannel = new Object();
        registry.activateRoot(activation(root, incarnation, 101));
        check(drain(registry).isEmpty(), "activation without windows has no batch");

        registry.upsert(app(app, appChannel, root, incarnation, 101, true, 0, 1, 720));
        List<WindowFocusRegistry.Publication> events = drain(registry);
        check(events.size() == 2 && events.get(0).kind == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "initial geometry then gain");
        checkFocus(events.get(1), app, appChannel, 1, true);

        // Non-focusable and non-winner windows still publish geometry, but no focus event.
        registry.upsert(popup(popup, popupChannel, app, root, incarnation, 101, true,
                WindowFocusRegistry.FLAG_NOT_FOCUSABLE, 2));
        events = drain(registry);
        check(events.size() == 1 && events.get(0).kind == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "nonfocusable geometry publication");
        check(events.get(0).flags == WindowFocusRegistry.FLAG_NOT_FOCUSABLE
                && events.get(0).order == 2
                && events.get(0).role == WindowFocusRegistry.WindowRole.ATTACHED,
                "geometry retains stacking and eligibility metadata");

        // A focused-window resize publishes geometry without stealing/re-gaining focus.
        registry.upsert(app(app, appChannel, root, incarnation, 101, true, 0, 1, 600));
        events = drain(registry);
        check(events.size() == 1 && events.get(0).kind == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "same winner resize only geometry");
        check(registry.focusSnapshot(0).epoch == 1, "resize does not create focus epoch");

        registry.upsert(popup(popup, popupChannel, app, root, incarnation, 101, true, 0, 2));
        events = drain(registry);
        check(events.size() == 3 && events.get(0).kind == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "popup geometry is not duplicated on gain");
        checkFocus(events.get(1), app, appChannel, 2, false);
        checkFocus(events.get(2), popup, popupChannel, 2, true);

        registry.upsert(popup(popup, popupChannel, app, root, incarnation, 101, false, 0, 2));
        events = drain(registry);
        check(events.size() == 3 && events.get(0).kind == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "hidden popup geometry then restoration");
        checkFocus(events.get(1), popup, popupChannel, 3, false);
        checkFocus(events.get(2), app, appChannel, 3, true);
        registry.remove(popup, popupChannel);
        events = drain(registry);
        check(events.size() == 1 && !events.get(0).visible && events.get(0).right == 0,
                "exact removal publishes hide geometry");
        registry.upsert(app(app, appChannel, root, incarnation, 101, true, 0, 1, 600));
        check(drain(registry).isEmpty(), "duplicate state remains no-op");
    }

    private static void testOrderIdentityAndActivation() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object root = new Object(), incarnation = new Object();
        Object app = new Object(), appChannel = new Object();
        Object popup = new Object(), popupChannel = new Object();
        registry.activateRoot(activation(root, incarnation, 202));
        registry.upsert(app(app, appChannel, root, incarnation, 202, true, 0, 5, 720));
        drain(registry);
        registry.upsert(popup(popup, popupChannel, app, root, incarnation, 202, true, 0, 10));
        drain(registry);
        check(registry.focusedWindow(0).windowToken == popup, "newer order wins");
        // Same window+channel keeps its insertion tie-order across resize/update.
        registry.upsert(popup(popup, popupChannel, app, root, incarnation, 202, true, 0, 1));
        drain(registry);
        check(registry.focusedWindow(0).windowToken == app,
                "explicit older order can relinquish focus");
        registry.upsert(popup(popup, popupChannel, app, root, incarnation, 202, true, 0, 5));
        drain(registry);
        check(registry.focusedWindow(0).windowToken == popup,
                "same-order tie retains original insertion precedence");

        Object oldIncarnation = new Object(), newIncarnation = new Object();
        WindowFocusRegistry.Activation old = activation(root, oldIncarnation, 202);
        WindowFocusRegistry.Activation next = activation(root, newIncarnation, 202);
        registry.upsert(app(app, appChannel, root, oldIncarnation, 202, true, 0, 10, 720));
        drain(registry);
        registry.upsert(app(new Object(), new Object(), root, newIncarnation, 202, true, 0, 1, 720));
        drain(registry);
        registry.activateRoot(old);
        drain(registry);
        registry.activateRoot(next);
        List<WindowFocusRegistry.Publication> switched = drain(registry);
        check(switched.size() == 2 && switched.get(0).epoch == switched.get(1).epoch,
                "activation loss/gain shares epoch");
        int count = switched.size();
        registry.resignRoot(old);
        check(drain(registry).isEmpty() && count == switched.size(), "late old resign ignored");
        registry.resignRoot(next);
        drain(registry);
        check(registry.focusedWindow(0) == null, "current activation resigns");
    }

    private static void testRootMismatchAndRetainedBatches() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object rootA = new Object(), rootB = new Object();
        Object incarnationA = new Object(), incarnationB = new Object();
        Object window = new Object(), channel = new Object();
        registry.activateRoot(activation(rootA, incarnationA, 303));
        registry.upsert(app(window, channel, rootB, incarnationB, 303, true, 0, 1, 720));
        WindowFocusRegistry.PublicationBatch first = registry.pendingBatch();
        check(first != null && first.publications.size() == 1, "root-mismatch geometry retained");
        registry.upsert(app(window, channel, rootA, incarnationA, 303, true, 0, 1, 720));
        WindowFocusRegistry.PublicationBatch second = registry.pendingBatch();
        check(second == first, "pending head retained while owner is outside lock");
        check(registry.ackAccepted(first), "first batch accepted");
        second = registry.pendingBatch();
        check(second != null && !registry.ackAccepted(first), "old batch cannot ack suffix");
        check(registry.ackAccepted(second), "second batch accepted");
        check(registry.focusedWindow(0).windowToken == window, "matching root gains focus");

        Object thirdChannel = new Object();
        registry.upsert(app(window, thirdChannel, rootA, incarnationA, 303, true, 0, 1, 720));
        List<WindowFocusRegistry.Publication> replacement = drain(registry);
        check(replacement.size() == 4 && !replacement.get(0).visible
                && replacement.get(0).channelIncarnation == channel
                && replacement.get(1).channelIncarnation == thirdChannel
                && replacement.get(1).kind
                == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "channel replacement geometry plus focus transition");
        check(replacement.get(2).epoch == replacement.get(3).epoch,
                "channel replacement loss/gain epoch");
    }

    private static void testImmutableActivationSnapshot() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        final Object root = new Object();
        final Object incarnation = new Object();
        final Object replacementIncarnation = new Object();
        final int[] pid = {404};
        WindowFocusRegistry.Activation captured = activation(root, incarnation, pid[0]);
        registry.activateRoot(captured);
        pid[0] = 999;
        registry.upsert(app(new Object(), new Object(), root, incarnation, 404, true, 0, 1, 720));
        drain(registry);
        check(registry.focusedWindow(0) != null && registry.activeRoot(0).pid() == 404,
                "immutable activation unaffected by provider state mutation");
        registry.resignRoot(activation(root, replacementIncarnation, 404));
        check(registry.focusedWindow(0) != null, "mismatched replacement resign ignored");
    }

    private static void testCrossDisplayMoveBatch() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object root0 = new Object(), root1 = new Object();
        Object inc0 = new Object(), inc1 = new Object();
        Object window = new Object(), channel = new Object();
        registry.activateRoot(activation(root0, inc0, 505));
        registry.activateRoot(new WindowFocusRegistry.Activation(root1, inc1, 505, 1));
        registry.upsert(app(window, channel, root0, inc0, 505, true, 0, 1, 720));
        drain(registry);
        WindowFocusRegistry.WindowSpec moved = new WindowFocusRegistry.WindowSpec(window, channel,
                505, root1, inc1, 1, null, WindowFocusRegistry.WindowRole.APPLICATION, 0,
                true, 0, 0, 720, 1280, 1);
        registry.upsert(moved);
        List<WindowFocusRegistry.Publication> events = drain(registry);
        check(events.size() == 4 && !events.get(0).visible
                && events.get(0).displayId == 0 && events.get(1).displayId == 1,
                "cross-display move invalidates old geometry before new geometry");
        check(events.get(2).epoch == events.get(3).epoch,
                "cross-display move loss/gain shares epoch");
    }

    private static void testUnboundSnapshotsAndLaterBinding() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object root = new Object(), incarnation = new Object();
        Object window = new Object(), channel = new Object();
        registry.activateRoot(activation(root, incarnation, 606));

        // Binding can legitimately lag geometry admission.  The unbound
        // record publishes geometry, but never creates a synthetic root or
        // focus transition merely because its pid matches the active root.
        registry.upsert(unboundApp(window, channel, 606, true, 0, 10, 720));
        WindowFocusRegistry.PublicationBatch unboundBatch = registry.pendingBatch();
        check(unboundBatch != null && unboundBatch.publications.size() == 1,
                "unbound snapshot retains geometry batch");
        check(unboundBatch.publications.get(0).rootToken == null
                && unboundBatch.publications.get(0).rootIncarnation == null,
                "unbound geometry preserves null root pair");
        check(registry.focusedWindow(0) == null, "unbound snapshot cannot focus");
        check(registry.activeRoot(0) != null && registry.activeRoot(0).rootToken() == root,
                "unbound snapshot does not synthesize root activation");

        // Keep the original FIFO head outstanding while the same canonical
        // window/channel receives its authenticated binding.
        registry.upsert(app(window, channel, root, incarnation, 606, true, 0, 10, 720));
        check(registry.pendingBatch() == unboundBatch,
                "later binding preserves existing publication FIFO");
        List<WindowFocusRegistry.Publication> events = drain(registry);
        check(events.size() == 3 && events.get(0).kind
                == WindowFocusRegistry.PublicationKind.GEOMETRY
                && events.get(1).kind == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "binding preserves ordered geometry before focus");
        checkFocus(events.get(2), window, channel, 1, true);
        check(registry.focusedWindow(0).windowToken == window,
                "actual root binding permits focus");
    }

    private static void testUnboundRootPairValidation() {
        Object window = new Object(), channel = new Object(), root = new Object();
        boolean tokenOnlyRejected = false;
        try {
            app(window, channel, root, null, 707, true, 0, 1, 720);
        } catch (IllegalArgumentException expected) {
            tokenOnlyRejected = true;
        }
        boolean incarnationOnlyRejected = false;
        try {
            app(window, channel, null, root, 707, true, 0, 1, 720);
        } catch (IllegalArgumentException expected) {
            incarnationOnlyRejected = true;
        }
        check(tokenOnlyRejected && incarnationOnlyRejected,
                "exactly one null root identity is rejected");
    }

    private static void testReferencesChannelAcrossReplacementBatches() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object window = new Object();
        Object oldChannel = new String("same-channel-value");
        Object successorChannel = new String("same-channel-value");
        registry.upsert(unboundApp(window, oldChannel, 808, true, 0, 1, 720));
        WindowFocusRegistry.PublicationBatch original = registry.pendingBatch();
        check(registry.referencesChannel(oldChannel),
                "current/pending original channel is referenced");

        registry.upsert(unboundApp(window, successorChannel, 808, true, 0, 1, 720));
        check(registry.pendingBatch() == original,
                "replacement appends successor batch behind original");
        check(registry.ackAccepted(original), "original replacement batch acknowledged");
        WindowFocusRegistry.PublicationBatch successor = registry.pendingBatch();
        check(successor != null && successor.publications.size() == 2
                && registry.referencesChannel(oldChannel)
                && registry.referencesChannel(successorChannel),
                "all pending batches and current successor are scanned");
        check(registry.referencesChannel(oldChannel),
                "retired original remains referenced by successor hide publication");
        check(registry.ackAccepted(registry.pendingBatch()), "successor batch acknowledged");
        check(!registry.referencesChannel(oldChannel),
                "stale original channel no longer references successor");
        check(registry.referencesChannel(successorChannel),
                "current successor channel remains referenced");
        check(!registry.referencesChannel(new String("same-channel-value")),
                "channel references use exact identity, not value equality");
    }

    private static void testDecisionRevocationEpochs() {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object root = new Object(), incarnation = new Object();
        Object window = new Object(), channel = new Object();
        WindowFocusRegistry.DecisionSnapshot initial = registry.decisionSnapshot(0);
        check(initial.window == null && initial.epoch == 0, "initial decision is not a grant");
        registry.activateRoot(activation(root, incarnation, 909));
        check(registry.decisionSnapshot(0).epoch == 0,
                "root activation without winner does not invent an epoch");
        registry.upsert(app(window, channel, root, incarnation, 909, true, 0, 1, 720));
        WindowFocusRegistry.DecisionSnapshot grant = registry.decisionSnapshot(0);
        check(grant.window.channelIncarnation == channel && grant.epoch == 1,
                "decision selects original channel at publication epoch");
        registry.upsert(app(window, channel, root, incarnation, 909, true, 0, 1, 600));
        check(registry.decisionSnapshot(0).epoch == 1, "geometry preserves decision epoch");
        registry.resignRoot(activation(root, incarnation, 909));
        WindowFocusRegistry.DecisionSnapshot loss = registry.decisionSnapshot(0);
        check(loss.window == null && loss.epoch == 2 && registry.focusSnapshot(0) == null,
                "revocation retains loss epoch without changing replay API");
        check(grant.window.channelIncarnation == channel && grant.epoch == 1,
                "retained decision is immutable after revocation");
        drain(registry);
        check(registry.decisionSnapshot(0).epoch == 2,
                "settlement does not erase authoritative revocation");
        registry.resignRoot(activation(root, incarnation, 909));
        check(registry.decisionSnapshot(0).epoch == 2, "duplicate resign is a no-op");
        Object otherRoot = new Object(), otherIncarnation = new Object();
        registry.activateRoot(new WindowFocusRegistry.Activation(otherRoot, otherIncarnation, 910, 1));
        registry.upsert(new WindowFocusRegistry.WindowSpec(new Object(), new Object(), 910,
                otherRoot, otherIncarnation, 1, null, WindowFocusRegistry.WindowRole.APPLICATION,
                0, true, 0, 0, 720, 1280, 1));
        check(registry.decisionSnapshot(1).epoch == 3 && registry.decisionSnapshot(0).epoch == 2,
                "other display transition cannot relabel this display's loss");
        registry.activateRoot(activation(root, incarnation, 909));
        check(registry.decisionSnapshot(0).epoch == 4
                && registry.decisionSnapshot(0).window.channelIncarnation == channel,
                "reactivation selects exact original with a new epoch");
    }

    public static void main(String[] args) {
        testGeometryAndPopupRestoration();
        testOrderIdentityAndActivation();
        testRootMismatchAndRetainedBatches();
        testImmutableActivationSnapshot();
        testCrossDisplayMoveBatch();
        testUnboundSnapshotsAndLaterBinding();
        testUnboundRootPairValidation();
        testReferencesChannelAcrossReplacementBatches();
        testDecisionRevocationEpochs();
        System.out.println("WMS retained publication batches and exact-root focus policy: PASS");
    }
}
