package dev.darwinart.runtime.wm;

import java.util.Arrays;

public final class WindowFocusSettlementGuardsTest {
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static WindowFocusRegistry registry() {
        WindowFocusRegistry value = new WindowFocusRegistry();
        add(value);
        return value;
    }

    private static void add(WindowFocusRegistry value) {
        value.upsert(new WindowFocusRegistry.WindowSpec(new Object(), new Object(), 77,
                new Object(), new Object(), 0, null, WindowFocusRegistry.WindowRole.APPLICATION,
                0, true, 0, 0, 360, 640, 1));
    }

    private static void defaultTerminationCannotDiscard() {
        WindowFocusRegistry registry = registry();
        int[] calls = {0};
        WindowFocusPublicationDelivery delivery = new WindowFocusPublicationDelivery(
                registry, publication -> {
                    ++calls[0];
                    return WindowFocusPublicationDelivery.Result.TERMINAL;
                });
        delivery.drain(8);
        WindowFocusPublicationDelivery.Attempt attempt = delivery.pendingTerminalAttempt();
        check(delivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.NOT_QUIESCENT,
                "default transport cannot assert termination");
        check(delivery.pendingTerminalAttempt() == attempt, "default preserves exact token");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL
                && calls[0] == 1, "default never replays quarantined record");
    }

    private static void headChangedDuringTermination() {
        WindowFocusRegistry registry = registry();
        WindowFocusRegistry.PublicationBatch old = registry.pendingBatch();
        int[] terminations = {0};
        WindowFocusPublicationDelivery delivery = new WindowFocusPublicationDelivery(
                registry, new WindowFocusPublicationDelivery.Transport() {
            @Override public WindowFocusPublicationDelivery.Result publish(
                    WindowFocusRegistry.Publication value) {
                return WindowFocusPublicationDelivery.Result.TERMINAL;
            }
            @Override public boolean terminateAndQuiesce(WindowFocusRegistry.Publication value) {
                ++terminations[0];
                check(value == old.publications.get(0), "termination has exact original");
                check(registry.ackAccepted(old), "test external owner changes original head");
                add(registry);
                return true;
            }
        });
        delivery.drain(8);
        WindowFocusPublicationDelivery.Attempt attempt = delivery.pendingTerminalAttempt();
        boolean rejected = false;
        try { delivery.settleTerminal(attempt); }
        catch (IllegalStateException expected) { rejected = true; }
        WindowFocusRegistry.PublicationBatch successor = registry.pendingBatch();
        check(rejected && successor != null && successor != old,
                "changed head rejected after external callback");
        check(delivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.INVALID,
                "old attempt cannot settle successor");
        check(registry.pendingBatch() == successor && terminations[0] == 1,
                "successor not consumed or terminated");
    }

    private static void malformedDispositionCannotAcknowledge() {
        WindowFocusRegistry registry = registry();
        WindowFocusRegistry.PublicationBatch batch = registry.pendingBatch();
        check(!registry.ackSettled(batch, null), "null outcomes rejected");
        check(!registry.ackSettled(batch, Arrays.asList(
                (WindowFocusRegistry.Disposition) null)), "unset outcome rejected");
        check(!registry.ackSettled(batch, Arrays.asList(WindowFocusRegistry.Disposition.ACCEPTED,
                WindowFocusRegistry.Disposition.ABANDONED_TERMINAL)), "wrong count rejected");
        check(registry.pendingBatch() == batch, "malformed outcomes preserve FIFO");
        check(registry.ackSettled(batch,
                Arrays.asList(WindowFocusRegistry.Disposition.ABANDONED_TERMINAL)),
                "explicit terminal abandonment is distinct from accepted");
        add(registry);
        WindowFocusRegistry.PublicationBatch successor = registry.pendingBatch();
        check(!registry.ackSettled(batch,
                Arrays.asList(WindowFocusRegistry.Disposition.ACCEPTED))
                && registry.pendingBatch() == successor, "stale outcomes cannot skip successor");
    }

    private static void errorCannotReopenPublication() {
        WindowFocusRegistry registry = registry();
        int[] calls = {0};
        WindowFocusPublicationDelivery delivery = new WindowFocusPublicationDelivery(
                registry, publication -> {
                    ++calls[0];
                    throw new AssertionError("uncertain external send");
                });
        boolean threw = false;
        try { delivery.drain(8); } catch (AssertionError expected) { threw = true; }
        check(threw && delivery.quarantineReason()
                == WindowFocusPublicationDelivery.QuarantineReason.THROWABLE,
                "Error propagates with uncertain quarantine");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL
                && calls[0] == 1, "Error never permits blind replay");
    }

    @SuppressWarnings("unchecked")
    private static <Failure extends Throwable, Value> Value throwUnchecked(Throwable value)
            throws Failure {
        throw (Failure) value;
    }

    private static void checkedThrowableRetainsQuarantine() {
        WindowFocusRegistry registry = registry();
        Exception sendFailure = new Exception("unchecked external checked exception");
        Exception terminationFailure = new Exception("unchecked termination exception");
        int[] sends = {0}, terminations = {0};
        WindowFocusPublicationDelivery delivery = new WindowFocusPublicationDelivery(
                registry, new WindowFocusPublicationDelivery.Transport() {
            @Override public WindowFocusPublicationDelivery.Result publish(
                    WindowFocusRegistry.Publication value) {
                ++sends[0];
                return WindowFocusSettlementGuardsTest
                        .<RuntimeException, WindowFocusPublicationDelivery.Result>
                                throwUnchecked(sendFailure);
            }
            @Override public boolean terminateAndQuiesce(WindowFocusRegistry.Publication value) {
                if (++terminations[0] == 1)
                    return WindowFocusSettlementGuardsTest
                            .<RuntimeException, Boolean>throwUnchecked(terminationFailure);
                return true;
            }
        });
        Throwable observed = null;
        try { delivery.drain(8); } catch (Throwable failure) { observed = failure; }
        check(observed == sendFailure, "original checked send exception propagated");
        WindowFocusPublicationDelivery.Attempt attempt = delivery.pendingTerminalAttempt();
        check(attempt != null, "checked exception exposes quarantined exact attempt");
        observed = null;
        try { delivery.settleTerminal(attempt); } catch (Throwable failure) { observed = failure; }
        check(observed == terminationFailure && delivery.pendingTerminalAttempt() == attempt,
                "checked termination failure preserves identical token");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL
                && sends[0] == 1, "checked exception does not reopen publication");
        check(delivery.settleTerminal(attempt)
                == WindowFocusPublicationDelivery.SettlementResult.SETTLED,
                "later real termination can settle checked exception");
    }

    public static void main(String[] args) {
        defaultTerminationCannotDiscard();
        headChangedDuringTermination();
        malformedDispositionCannotAcknowledge();
        errorCannotReopenPublication();
        checkedThrowableRetainsQuarantine();
        System.out.println("WMS terminal settlement default/changed-head/outcome/Error guards: PASS");
    }
}
