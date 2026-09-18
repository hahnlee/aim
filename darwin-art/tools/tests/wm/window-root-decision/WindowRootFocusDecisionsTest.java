package dev.darwinart.runtime.wm;

import android.os.DeadObjectException;
import android.os.IBinder;
import android.os.RemoteException;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Fixture for the actual WindowRootFocusDecisions owner. */
public final class WindowRootFocusDecisionsTest {
    private static final class Token implements IBinder {}

    private static final class Callback implements IBinder, IBinder.DecisionEndpoint {
        final List<DesktopRootFocusDecision> received = new ArrayList<>();
        int remoteFailures;
        boolean deadObject;
        RuntimeException rejection;
        Throwable blockedFailure;
        CountDownLatch entered;
        CountDownLatch release;
        Runnable duringSend;

        @Override public boolean send(DesktopRootFocusDecision decision) throws RemoteException {
            received.add(decision);
            if (duringSend != null) {
                Runnable action = duringSend;
                duringSend = null;
                action.run();
            }
            if (entered != null) {
                entered.countDown();
                try {
                    if (!release.await(2L, TimeUnit.SECONDS))
                        throw new RemoteException("blocked fixture timed out");
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    throw new RemoteException("blocked fixture interrupted");
                }
                if (blockedFailure instanceof RemoteException)
                    throw (RemoteException) blockedFailure;
                if (blockedFailure instanceof RuntimeException)
                    throw (RuntimeException) blockedFailure;
            }
            if (deadObject) throw new DeadObjectException("callback is gone");
            if (rejection != null) throw rejection;
            if (remoteFailures != 0) {
                --remoteFailures;
                throw new RemoteException("transient callback failure");
            }
            return true;
        }
    }

    private static final class Fixture {
        final Object monitor = new Object();
        final WindowFocusRegistry registry = new WindowFocusRegistry();
        final WindowRootActivationPolicy activation = new WindowRootActivationPolicy(registry);
        final WindowPublicationDriver driver = new WindowPublicationDriver();
        final DesktopRootRegistry.Registration root = new DesktopRootRegistry.Registration(41L);
        final Callback callback = new Callback();
        final IBinder token = new Token();
        final WindowRootFocusDecisions owner;

        Fixture() {
            DesktopRootRegistry.Fact fact = new DesktopRootRegistry.Fact(7L);
            root.latestFact = fact;
            activation.activate(root, fact.serial);
            registry.select(root, 1L);
            owner = new WindowRootFocusDecisions(monitor, registry, activation, driver,
                    selected -> token);
        }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void resetScheduler() { WindowRootFocusDecisionDelivery.clear(); }

    private static void testTransientRemoteRetryKeepsExactDecision() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.callback.remoteFailures = 1;
        fixture.owner.attach(fixture.root, fixture.callback);
        check(WindowRootFocusDecisionDelivery.size() == 1, "initial decision was not scheduled");
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.size() == 1, "first delivery missing");
        check(WindowRootFocusDecisionDelivery.size() == 1
                        && WindowRootFocusDecisionDelivery.nextDelay() == 4,
                "transient RemoteException did not schedule bounded retry");
        DesktopRootFocusDecision first = fixture.callback.received.get(0);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.size() == 2, "retry delivery missing");
        check(fixture.callback.received.get(1) == first,
                "retry did not retain the exact immutable decision");
        check(WindowRootFocusDecisionDelivery.size() == 0, "accepted retry remained pending");
    }

    private static void testAcceptedSuccessorPreservesNewerSnapshot() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.callback.duringSend = () -> {
            fixture.registry.select(fixture.root, 2L);
            fixture.owner.captureAll();
        };
        fixture.owner.attach(fixture.root, fixture.callback);
        WindowRootFocusDecisionDelivery.runNext();
        DesktopRootFocusDecision first = fixture.callback.received.get(0);
        check(fixture.callback.received.size() == 1,
                "successor was delivered reentrantly instead of being retained");
        check(WindowRootFocusDecisionDelivery.size() == 1
                        && WindowRootFocusDecisionDelivery.nextDelay() == 0,
                "accepted predecessor discarded newer pending snapshot");
        WindowRootFocusDecisionDelivery.runNext();
        DesktopRootFocusDecision successor = fixture.callback.received.get(1);
        check(successor != first && successor.sequence == 2L && successor.epoch == 2L,
                "newer decision did not survive predecessor completion");
        check(successor.originalChannelToken == fixture.token,
                "newer decision lost the immutable original channel token");
        check(WindowRootFocusDecisionDelivery.size() == 0, "successor remained pending");
    }

    private static void testRetireBeforeQueuedDispatch() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.owner.attach(fixture.root, fixture.callback);
        fixture.owner.retire(fixture.root);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.isEmpty(), "retired root received a callback");
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "retired root subscription was retained for retry");
    }

    private static void testTerminalBeforeQueuedDispatchWithoutRetire() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.owner.attach(fixture.root, fixture.callback);
        fixture.root.terminal = true;
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.isEmpty(),
                "terminal root received a callback without explicit retire");
        fixture.owner.captureAll();
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "captureAll retained terminal root subscription");
    }

    private static void testRetireDuringInFlightCompletion() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.callback.duringSend = () -> {
            fixture.owner.retire(fixture.root);
        };
        fixture.owner.attach(fixture.root, fixture.callback);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.size() == 1, "in-flight callback missing");
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "retired in-flight completion scheduled a retry");
    }

    private static void testTerminalDuringSendStopsAllFailures() {
        Throwable[] failures = {
            new RemoteException("closed transient callback"),
            new DeadObjectException("closed callback object"),
            new IllegalStateException("closed callback rejection")
        };
        for (Throwable failure : failures) {
            resetScheduler();
            Fixture fixture = new Fixture();
            fixture.callback.blockedFailure = failure;
            fixture.callback.duringSend = () -> fixture.root.terminal = true;
            fixture.owner.attach(fixture.root, fixture.callback);
            WindowRootFocusDecisionDelivery.runNext();
            check(fixture.callback.received.size() == 1,
                    "closed callback was not attempted exactly once");
            check(WindowRootFocusDecisionDelivery.size() == 0,
                    "closed callback failure scheduled a retry: " + failure.getClass());
            check(!fixture.root.clientDied, "closed callback fabricated client death");
        }
    }

    private static void testBlockedDeadObjectAfterMonitorRetirement() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.callback.entered = new CountDownLatch(1);
        fixture.callback.release = new CountDownLatch(1);
        fixture.callback.blockedFailure = new DeadObjectException("blocked callback object");
        fixture.owner.attach(fixture.root, fixture.callback);
        Thread delivery = new Thread(WindowRootFocusDecisionDelivery::runNext,
                "window-root-blocked-delivery-test");
        delivery.start();
        try {
            check(fixture.callback.entered.await(2L, TimeUnit.SECONDS),
                    "blocked callback did not enter transport");
            synchronized (fixture.monitor) {
                fixture.root.terminal = true;
                fixture.owner.retire(fixture.root);
            }
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError("blocked callback wait interrupted", interrupted);
        } finally {
            fixture.callback.release.countDown();
        }
        try {
            delivery.join(2_000L);
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError("blocked callback join interrupted", interrupted);
        }
        check(!delivery.isAlive(), "blocked callback delivery did not quiesce");
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "blocked DeadObject completion scheduled a retry");
        check(!fixture.root.clientDied, "blocked DeadObject fabricated client death");
    }

    private static void testRetiringOldRootLeavesSuccessor() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.owner.attach(fixture.root, fixture.callback);
        DesktopRootRegistry.Registration successor =
                new DesktopRootRegistry.Registration(42L);
        DesktopRootRegistry.Fact successorFact = new DesktopRootRegistry.Fact(8L);
        successor.latestFact = successorFact;
        Callback successorCallback = new Callback();
        fixture.activation.activate(successor, successorFact.serial);
        fixture.registry.select(successor, 2L);
        fixture.owner.attach(successor, successorCallback);
        fixture.owner.retire(fixture.root);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.isEmpty(), "retired predecessor was delivered");
        WindowRootFocusDecisionDelivery.runNext();
        check(successorCallback.received.size() == 1,
                "retiring predecessor removed the successor registration");
        check(successorCallback.received.get(0).incarnation == successor.incarnation,
                "successor callback received predecessor identity");
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "successor registration remained pending");
    }

    private static void testDeadObjectAfterRetireDoesNotFabricateDeath() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.callback.deadObject = true;
        fixture.owner.attach(fixture.root, fixture.callback);
        fixture.owner.retire(fixture.root);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.isEmpty(),
                "DeadObject callback ran after explicit subscription retirement");
        check(!fixture.root.clientDied && fixture.root.bindingOpen(),
                "retirement fabricated authenticated root/client death");
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "DeadObject after retirement scheduled a callback retry");
    }

    private static void testNullSelectionRevocationRetriesExactDecision() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.registry.select(null, 9L);
        fixture.callback.remoteFailures = 1;
        fixture.owner.attach(fixture.root, fixture.callback);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.size() == 1, "revocation was not delivered");
        DesktopRootFocusDecision revoke = fixture.callback.received.get(0);
        check(revoke.originalChannelToken == null && revoke.epoch == 9L,
                "null selection did not produce an ordered revocation");
        check(WindowRootFocusDecisionDelivery.size() == 1
                        && WindowRootFocusDecisionDelivery.nextDelay() == 4,
                "revocation RemoteException did not schedule retry");
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.get(1) == revoke,
                "revocation retry changed the immutable decision");
        check(WindowRootFocusDecisionDelivery.size() == 0, "revocation retry remained pending");
    }

    private static void testRejectedCallbackAfterRetireDoesNotRetry() {
        resetScheduler();
        Fixture fixture = new Fixture();
        fixture.callback.rejection = new IllegalStateException("callback rejected");
        fixture.owner.attach(fixture.root, fixture.callback);
        fixture.owner.retire(fixture.root);
        WindowRootFocusDecisionDelivery.runNext();
        check(fixture.callback.received.isEmpty(),
                "rejected callback ran after explicit subscription retirement");
        check(WindowRootFocusDecisionDelivery.size() == 0,
                "rejected callback after retirement scheduled a retry");
    }

    public static void main(String[] args) {
        testTransientRemoteRetryKeepsExactDecision();
        testAcceptedSuccessorPreservesNewerSnapshot();
        testRetireBeforeQueuedDispatch();
        testTerminalBeforeQueuedDispatchWithoutRetire();
        testRetireDuringInFlightCompletion();
        testTerminalDuringSendStopsAllFailures();
        testBlockedDeadObjectAfterMonitorRetirement();
        testRetiringOldRootLeavesSuccessor();
        testDeadObjectAfterRetireDoesNotFabricateDeath();
        testRejectedCallbackAfterRetireDoesNotRetry();
        testNullSelectionRevocationRetriesExactDecision();
        System.out.println("window-root decision delivery/retry/death fixture: PASS");
    }
}
