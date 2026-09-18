package dev.darwinart.runtime.wm;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Independent integration of exclusive delivery admission and terminal settlement. */
public final class WindowFocusSettlementConcurrencyTest {
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    public static void main(String[] args) throws Exception {
        WindowFocusRegistry registry = new WindowFocusRegistry();
        Object root = new Object(), incarnation = new Object();
        Object window = new Object(), channel = new Object();
        registry.upsert(new WindowFocusRegistry.WindowSpec(window, channel, 77,
                root, incarnation, 0, null, WindowFocusRegistry.WindowRole.APPLICATION,
                0, true, 0, 0, 360, 640, 1));
        WindowFocusRegistry.PublicationBatch original = registry.pendingBatch();
        CountDownLatch entered = new CountDownLatch(1), release = new CountDownLatch(1);
        int[] publishes = {0}, terminations = {0};
        WindowFocusPublicationDelivery.Transport transport =
                new WindowFocusPublicationDelivery.Transport() {
            @Override public WindowFocusPublicationDelivery.Result publish(
                    WindowFocusRegistry.Publication publication) {
                ++publishes[0];
                return publishes[0] == 1 ? WindowFocusPublicationDelivery.Result.TERMINAL
                        : WindowFocusPublicationDelivery.Result.ACCEPTED;
            }
            @Override public boolean terminateAndQuiesce(
                    WindowFocusRegistry.Publication publication) {
                ++terminations[0];
                check(publication == original.publications.get(0), "original record terminated");
                check(publication.channelIncarnation == channel, "original channel terminated");
                entered.countDown();
                try {
                    check(release.await(5, TimeUnit.SECONDS), "termination release timeout");
                } catch (InterruptedException failure) {
                    Thread.currentThread().interrupt();
                    throw new AssertionError(failure);
                }
                return true;
            }
        };
        WindowFocusPublicationDelivery delivery =
                new WindowFocusPublicationDelivery(registry, transport);
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.TERMINAL,
                "original attempt quarantined");
        WindowFocusPublicationDelivery.Attempt attempt = delivery.pendingTerminalAttempt();
        check(attempt != null, "opaque attempt exists");
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread settlement = new Thread(() -> {
            try { delivery.settleTerminal(attempt); }
            catch (Throwable error) { failure.set(error); }
        }, "wms-terminal-settlement");
        settlement.start();
        try {
            check(entered.await(5, TimeUnit.SECONDS), "termination not entered");
            // Both monitors must remain available while the provider is blocked.
            check(registry.pendingBatch() == original, "registry monitor available");
            check(delivery.pendingTerminalAttempt() == attempt, "delivery monitor available");
            registry.upsert(new WindowFocusRegistry.WindowSpec(window, new Object(), 77,
                    root, incarnation, 0, null, WindowFocusRegistry.WindowRole.APPLICATION,
                    0, true, 0, 0, 400, 640, 1));
            check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.COALESCED,
                    "concurrent drain coalesced");
            delivery.settleTerminal(attempt);
            check(terminations[0] == 1 && publishes[0] == 1,
                    "concurrent settlement cannot terminate or publish twice");
        } finally {
            release.countDown();
            settlement.join(5000);
        }
        check(!settlement.isAlive(), "settlement thread ended");
        if (failure.get() != null) throw new AssertionError(failure.get());
        check(delivery.pendingTerminalAttempt() == null, "quarantine settled");
        check(delivery.drain(8) == WindowFocusPublicationDelivery.DrainResult.DRAINED,
                "replacement publications can progress");
        check(terminations[0] == 1, "successor endpoint not terminated");
        System.out.println("WMS terminal settlement concurrency and original-endpoint isolation: PASS");
    }
}
