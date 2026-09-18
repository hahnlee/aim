package dev.darwinart.runtime.wm;

import java.util.ArrayList;
import java.util.List;

public final class WindowFocusPublicationDeliveryTest {
    private static final class ScriptedTransport
            implements WindowFocusPublicationDelivery.Transport {
        final List<WindowFocusRegistry.Publication> seen = new ArrayList<>();
        final List<WindowFocusPublicationDelivery.Result> script = new ArrayList<>();
        final List<Boolean> terminationScript = new ArrayList<>();
        WindowFocusPublicationDelivery delivery;
        WindowFocusPublicationDelivery.DrainResult nestedResult;
        Runnable onFirst;
        boolean enqueueOnFirst;
        boolean throwOnce;
        boolean throwTerminationOnce;
        boolean settleOnTermination;
        WindowFocusPublicationDelivery.SettlementResult nestedSettlement;

        @Override public boolean terminateAndQuiesce(
                WindowFocusRegistry.Publication exactOriginal) {
            if (throwTerminationOnce) {
                throwTerminationOnce = false;
                throw new IllegalStateException("termination failed");
            }
            if (settleOnTermination) {
                settleOnTermination = false;
                nestedSettlement = delivery.settleTerminal(delivery.pendingTerminalAttempt());
            }
            if (!terminationScript.isEmpty()) return terminationScript.remove(0);
            return false;
        }

        @Override public WindowFocusPublicationDelivery.Result publish(
                WindowFocusRegistry.Publication publication) {
            seen.add(publication);
            if (throwOnce) {
                throwOnce = false;
                throw new IllegalStateException("sink failed");
            }
            if (enqueueOnFirst && seen.size() == 1) {
                enqueueOnFirst = false;
                if (onFirst != null) onFirst.run();
                nestedResult = delivery.drain(8);
            }
            if (!script.isEmpty()) return script.remove(0);
            return WindowFocusPublicationDelivery.Result.ACCEPTED;
        }
    }

    private static final class Fixture {
        final WindowFocusRegistry registry = new WindowFocusRegistry();
        final Object root = new Object();
        final Object rootIncarnation = new Object();
        final Object window = new Object();
        final Object channel = new Object();

        Fixture() {
            registry.activateRoot(new WindowFocusRegistry.Activation(root, rootIncarnation, 77, 0));
            registry.upsert(new WindowFocusRegistry.WindowSpec(window, channel, 77, root,
                    rootIncarnation, 0, null, WindowFocusRegistry.WindowRole.APPLICATION,
                    0, true, 0, 0, 720, 1280, 1));
        }

        void resize(int right) {
            registry.upsert(new WindowFocusRegistry.WindowSpec(window, channel, 77, root,
                    rootIncarnation, 0, null, WindowFocusRegistry.WindowRole.APPLICATION,
                    0, true, 0, 0, right, 1280, 1));
        }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void testBlockedMiddleAndOrder() {
        Fixture fixture = new Fixture();
        ScriptedTransport sink = new ScriptedTransport();
        sink.script.add(WindowFocusPublicationDelivery.Result.ACCEPTED);
        sink.script.add(WindowFocusPublicationDelivery.Result.BACKPRESSURED);
        WindowFocusPublicationDelivery delivery =
                new WindowFocusPublicationDelivery(fixture.registry, sink);
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.BLOCKED,
                "middle record blocks");
        check(sink.seen.size() == 2 && sink.seen.get(0).kind
                == WindowFocusRegistry.PublicationKind.GEOMETRY,
                "ordered prefix delivered");
        check(fixture.registry.pendingBatch() != null, "blocked batch retained");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "blocked record resumes");
        check(sink.seen.size() == 3 && sink.seen.get(0) != sink.seen.get(1)
                && sink.seen.get(1) == sink.seen.get(2),
                "accepted prefix is not replayed");
    }

    private static void testTerminalAndBudget() {
        Fixture terminalFixture = new Fixture();
        ScriptedTransport terminalSink = new ScriptedTransport();
        terminalSink.script.add(WindowFocusPublicationDelivery.Result.TERMINAL);
        WindowFocusPublicationDelivery terminalDelivery =
                new WindowFocusPublicationDelivery(terminalFixture.registry, terminalSink);
        terminalSink.delivery = terminalDelivery;
        WindowFocusRegistry.PublicationBatch head = terminalFixture.registry.pendingBatch();
        check(terminalDelivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL,
                "terminal result visible");
        check(terminalFixture.registry.pendingBatch() == head, "terminal is not acknowledged");
        check(terminalDelivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL,
                "quarantined terminal is not replayed");
        WindowFocusPublicationDelivery.Attempt attempt = terminalDelivery.pendingTerminalAttempt();
        check(attempt != null, "terminal attempt is retained");
        terminalSink.terminationScript.add(false);
        check(terminalDelivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.NOT_QUIESCENT,
                "false termination leaves quarantine");
        check(terminalDelivery.pendingTerminalAttempt() == attempt, "same token remains latched");
        terminalSink.settleOnTermination = true;
        terminalSink.terminationScript.add(false);
        check(terminalDelivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.NOT_QUIESCENT,
                "reentrant termination remains owned by outer settlement");
        check(terminalSink.nestedSettlement
                == WindowFocusPublicationDelivery.SettlementResult.COALESCED,
                "reentrant settlement coalesces");
        terminalSink.throwTerminationOnce = true;
        boolean terminationThrew = false;
        try { terminalDelivery.settleTerminal(attempt); }
        catch (IllegalStateException expected) { terminationThrew = true; }
        check(terminationThrew && terminalDelivery.pendingTerminalAttempt() == attempt,
                "termination throw leaves same token quarantined");
        terminalSink.terminationScript.add(true);
        check(terminalDelivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.SETTLED,
                "true termination settles exact terminal");
        check(terminalDelivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "settled terminal batch drains without replay");
        check(terminalSink.seen.size() == 2 && terminalSink.seen.get(0) == head.publications.get(0),
                "terminal record published once and suffix continues");
        check(terminalDelivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.INVALID,
                "stale settled attempt rejected");

        Fixture foreignFixture = new Fixture();
        ScriptedTransport foreignSink = new ScriptedTransport();
        WindowFocusPublicationDelivery foreignDelivery =
                new WindowFocusPublicationDelivery(foreignFixture.registry, foreignSink);
        check(foreignDelivery.drain(1) == WindowFocusPublicationDelivery.DrainResult.BUDGET_EXHAUSTED,
                "foreign fixture first record accepted");
        check(foreignFixture.registry.pendingBatch() != null, "foreign fixture retains suffix");
        // A token from another owner and a stale token cannot settle anything.
        check(foreignDelivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.INVALID,
                "foreign attempt rejected");

        Fixture budgetFixture = new Fixture();
        ScriptedTransport budgetSink = new ScriptedTransport();
        WindowFocusPublicationDelivery budgetDelivery =
                new WindowFocusPublicationDelivery(budgetFixture.registry, budgetSink);
        check(budgetDelivery.drain(1)
                == WindowFocusPublicationDelivery.DrainResult.BUDGET_EXHAUSTED,
                "budget stops after one record");
        check(budgetDelivery.drain(1) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "budget resumes exact suffix");
        check(budgetSink.seen.size() == 2, "budget does not duplicate");
    }

    private static void testReentryAndNewBatch() {
        Fixture fixture = new Fixture();
        ScriptedTransport sink = new ScriptedTransport();
        WindowFocusPublicationDelivery delivery =
                new WindowFocusPublicationDelivery(fixture.registry, sink);
        sink.delivery = delivery;
        sink.onFirst = () -> fixture.resize(600);
        sink.enqueueOnFirst = true;
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "reentrant new batch drains");
        check(sink.nestedResult == WindowFocusPublicationDelivery.DrainResult.COALESCED
                && sink.seen.size() == 3, "reentrant drain coalesces and both batches deliver");
    }

    private static void testMixedAcceptedTerminalSuccessor() {
        Fixture fixture = new Fixture();
        ScriptedTransport sink = new ScriptedTransport();
        WindowFocusPublicationDelivery delivery =
                new WindowFocusPublicationDelivery(fixture.registry, sink);
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "mixed fixture starts drained");
        Object popup = new Object(), popupChannel = new Object();
        fixture.registry.upsert(new WindowFocusRegistry.WindowSpec(popup, popupChannel, 77,
                fixture.root, fixture.rootIncarnation, 0, fixture.window,
                WindowFocusRegistry.WindowRole.ATTACHED, 0, true, 10, 20, 200, 300, 2));
        WindowFocusRegistry.PublicationBatch batch = fixture.registry.pendingBatch();
        check(batch != null && batch.publications.size() == 3, "mixed batch has three records");
        sink.script.add(WindowFocusPublicationDelivery.Result.ACCEPTED);
        sink.script.add(WindowFocusPublicationDelivery.Result.TERMINAL);
        sink.script.add(WindowFocusPublicationDelivery.Result.ACCEPTED);
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL,
                "mixed batch quarantines terminal middle");
        WindowFocusPublicationDelivery.Attempt attempt = delivery.pendingTerminalAttempt();
        check(attempt != null, "mixed terminal attempt retained");
        check(sink.seen.size() == 4 && sink.seen.get(2) == batch.publications.get(0)
                && sink.seen.get(3) == batch.publications.get(1),
                "accepted prefix and terminal middle preserve FIFO");
        sink.terminationScript.add(true);
        check(delivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.SETTLED,
                "mixed terminal settles exactly");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "mixed live successor drains");
        check(sink.seen.size() == 5 && sink.seen.get(4) == batch.publications.get(2),
                "accepted prefix and terminal middle are never replayed");
    }

    private static void testFocusLossGainOrderAndNullResult() {
        Fixture fixture = new Fixture();
        ScriptedTransport sink = new ScriptedTransport();
        WindowFocusPublicationDelivery delivery =
                new WindowFocusPublicationDelivery(fixture.registry, sink);
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "initial batch drains");

        Object popup = new Object(), popupChannel = new Object();
        fixture.registry.upsert(new WindowFocusRegistry.WindowSpec(popup, popupChannel, 77,
                fixture.root, fixture.rootIncarnation, 0, fixture.window,
                WindowFocusRegistry.WindowRole.ATTACHED, 0, true, 10, 20, 200, 300, 2));
        WindowFocusRegistry.PublicationBatch transition = fixture.registry.pendingBatch();
        check(transition != null && transition.publications.size() == 3,
                "focus transition retains one ordered batch");
        check(transition.publications.get(1).kind == WindowFocusRegistry.PublicationKind.FOCUS
                && !transition.publications.get(1).focused
                && transition.publications.get(2).focused, "focus loss precedes gain");
        sink.script.add(null);
        boolean threw = false;
        try { delivery.drain(8); } catch (IllegalStateException expected) { threw = true; }
        check(threw && fixture.registry.pendingBatch() == transition,
                "null result throws without moving cursor");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL,
                "null-result batch remains quarantined");
        WindowFocusPublicationDelivery.Attempt attempt = delivery.pendingTerminalAttempt();
        check(attempt != null && delivery.quarantineReason()
                == WindowFocusPublicationDelivery.QuarantineReason.NULL_RESULT,
                "null result latches an uncertain attempt");
        sink.terminationScript.add(true);
        check(delivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.SETTLED,
                "null-result attempt settles after endpoint termination");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "settled null-result batch resumes suffix");
        check(sink.seen.size() == 5 && sink.seen.get(2) == transition.publications.get(0)
                && sink.seen.get(3) == transition.publications.get(1)
                && sink.seen.get(4) == transition.publications.get(2),
                "settled null-result batch never replays uncertain head");
    }

    private static void testThrowAndLostHead() {
        Fixture throwingFixture = new Fixture();
        ScriptedTransport throwingSink = new ScriptedTransport();
        throwingSink.throwOnce = true;
        WindowFocusPublicationDelivery throwingDelivery =
                new WindowFocusPublicationDelivery(throwingFixture.registry, throwingSink);
        boolean threw = false;
        try { throwingDelivery.drain(8); } catch (IllegalStateException expected) { threw = true; }
        check(threw, "sink throw propagates");
        check(throwingDelivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL,
                "throwing sink remains quarantined");
        WindowFocusPublicationDelivery.Attempt attempt = throwingDelivery.pendingTerminalAttempt();
        check(attempt != null && throwingDelivery.quarantineReason()
                == WindowFocusPublicationDelivery.QuarantineReason.THROWABLE,
                "throw latches an uncertain attempt");
        throwingSink.terminationScript.add(true);
        check(throwingDelivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.SETTLED,
                "throwing attempt settles after endpoint termination");
        check(throwingDelivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "settled throwing batch completes without replay");
        check(throwingSink.seen.size() == 2, "throwing record is never replayed");

        Fixture lostFixture = new Fixture();
        ScriptedTransport lostSink = new ScriptedTransport();
        lostSink.script.add(WindowFocusPublicationDelivery.Result.BACKPRESSURED);
        WindowFocusPublicationDelivery lostDelivery =
                new WindowFocusPublicationDelivery(lostFixture.registry, lostSink);
        check(lostDelivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.BLOCKED,
                "lost-head setup blocks");
        check(lostFixture.registry.ackAccepted(lostFixture.registry.pendingBatch()),
                "external owner removes exact head");
        threw = false;
        try { lostDelivery.drain(8); } catch (IllegalStateException expected) { threw = true; }
        check(threw, "lost head is not silently skipped");
    }

    private static void testArgumentsAndEmpty() {
        Fixture fixture = new Fixture();
        ScriptedTransport sink = new ScriptedTransport();
        WindowFocusPublicationDelivery delivery =
                new WindowFocusPublicationDelivery(fixture.registry, sink);
        boolean threw = false;
        try { delivery.drain(0); } catch (IllegalArgumentException expected) { threw = true; }
        check(threw, "zero budget rejected");
        check(new WindowFocusPublicationDelivery(new WindowFocusRegistry(), sink).drain(1)
                == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "empty registry drains");
        threw = false;
        try { new WindowFocusPublicationDelivery(null, sink); }
        catch (IllegalArgumentException expected) { threw = true; }
        check(threw, "null registry rejected");
        check(fixture.registry.pendingBatch() != null, "invalid argument did not mutate registry");
    }

    public static void main(String[] args) {
        testBlockedMiddleAndOrder();
        testTerminalAndBudget();
        testReentryAndNewBatch();
        testMixedAcceptedTerminalSuccessor();
        testFocusLossGainOrderAndNullResult();
        testThrowAndLostHead();
        testArgumentsAndEmpty();
        System.out.println("WMS exact publication delivery ownership, retry, reentry and failure: PASS");
    }
}
