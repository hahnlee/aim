package dev.darwinart.runtime.am;

import android.os.IBinder;
import java.lang.reflect.Field;
import java.lang.reflect.Proxy;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Isolated contract tests for the bound-service process launch ledger. */
public final class BoundServiceProcessLauncherTest {
    private static final String PACKAGE = "org.example.launcher";
    private static final String PROCESS = "org.example.launcher:service";
    private static final int UID = 10042;

    private static final class Backend implements BoundServiceProcessLauncher.Backend {
        ApplicationProcessRegistry registry;
        BoundServiceProcessLauncher launcher;
        Runnable duringActivate;
        int nextPid = 4100;
        long nextHandle = 71;
        int prepares;
        int activates;
        int cancels;
        int forgets;
        int aborts;
        boolean failActivate;
        boolean failCancel;
        boolean failForget;
        RuntimeException activationFailure;
        RuntimeException cancelFailure;
        RuntimeException forgetFailure;
        boolean invalidPreparation;
        int lastPid;
        long lastSequence;
        Runnable duringForget;
        Object launcherMonitor;
        CountDownLatch activateEntered;
        CountDownLatch releaseActivate;
        CountDownLatch cancelEntered;
        CountDownLatch releaseCancel;
        RuntimeException observedCancelFailure;
        RuntimeException observedForgetFailure;

        void checkUnlocked() {
            if (launcherMonitor != null && Thread.holdsLock(launcherMonitor)) {
                throw new AssertionError("backend transport called under launcher monitor");
            }
        }

        @Override public long[] prepare(String packageName, String processName, int uid,
                boolean isolated, long startSequence) {
            checkUnlocked();
            prepares++;
            lastPid = nextPid++;
            lastSequence = startSequence;
            return new long[] {invalidPreparation ? 0 : lastPid, nextHandle++};
        }

        @Override public void activate(long handle) {
            checkUnlocked();
            activates++;
            if (duringActivate != null) duringActivate.run();
            if (activateEntered != null) activateEntered.countDown();
            if (releaseActivate != null) await(releaseActivate);
            if (activationFailure != null) throw activationFailure;
            if (failActivate) throw new IllegalStateException("activate failed");
        }

        @Override public void abortPrepared(long handle) {
            checkUnlocked();
            aborts++;
        }

        @Override public void cancel(long handle) {
            checkUnlocked();
            cancels++;
            if (cancelEntered != null) cancelEntered.countDown();
            if (releaseCancel != null) await(releaseCancel);
            if (cancelFailure != null) {
                observedCancelFailure = cancelFailure;
                throw cancelFailure;
            }
            if (failCancel) throw new IllegalStateException("cancel failed");
        }

        @Override public void forget(long handle) {
            checkUnlocked();
            forgets++;
            if (duringForget != null) {
                Runnable callback = duringForget;
                duringForget = null;
                callback.run();
            }
            if (forgetFailure != null) {
                observedForgetFailure = forgetFailure;
                throw forgetFailure;
            }
            if (failForget) throw new IllegalStateException("forget failed");
        }
    }

    private static IBinder binder() {
        return (IBinder) Proxy.newProxyInstance(
                BoundServiceProcessLauncherTest.class.getClassLoader(),
                new Class<?>[] {IBinder.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("equals")) {
                        return args != null && args.length == 1 && proxy == args[0];
                    }
                    if (method.getName().equals("hashCode")) {
                        return System.identityHashCode(proxy);
                    }
                    if (method.getReturnType() == boolean.class) return false;
                    if (method.getReturnType() == int.class) return 0;
                    return null;
                });
    }

    private static void check(boolean condition) {
        if (!condition) throw new AssertionError("bound-service launcher assertion failed");
    }

    private static void checkSame(Object expected, Object actual) {
        if (expected != actual) {
            throw new AssertionError("bound-service launcher identity changed");
        }
    }

    private static void checkNoThrow(Thread thread) {
        try {
            thread.join(2000);
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError(interrupted);
        }
        check(!thread.isAlive());
    }

    private static void await(CountDownLatch latch) {
        try {
            check(latch.await(2, TimeUnit.SECONDS));
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError(interrupted);
        }
    }

    private static Object launcherMonitor(BoundServiceProcessLauncher launcher) {
        try {
            Field field = BoundServiceProcessLauncher.class.getDeclaredField("monitor");
            field.setAccessible(true);
            return field.get(launcher);
        } catch (ReflectiveOperationException reflectionFailure) {
            throw new AssertionError(reflectionFailure);
        }
    }

    private static BoundServiceProcessLauncher launcher(
            ApplicationProcessRegistry registry, Backend backend) {
        BoundServiceProcessLauncher launcher = new BoundServiceProcessLauncher(registry, backend);
        backend.launcherMonitor = launcherMonitor(launcher);
        return launcher;
    }

    private static RuntimeException expectFailure(Runnable operation) {
        try {
            operation.run();
        } catch (RuntimeException expected) {
            return expected;
        }
        throw new AssertionError("operation unexpectedly succeeded");
    }

    private static ApplicationProcessRegistry.AttachedApplication attach(
            ApplicationProcessRegistry registry, int pid, long sequence) {
        IBinder thread = binder();
        registry.beginAttachment(pid, UID, thread, sequence);
        registry.identify(pid, sequence, thread, PACKAGE);
        return registry.finishAttachment(pid, sequence);
    }

    private static ApplicationProcessRegistry.AttachedApplication attachAndRetire(
            ApplicationProcessRegistry registry, int pid, long sequence) {
        ApplicationProcessRegistry.AttachedApplication attached = attach(registry, pid, sequence);
        return registry.retireAttached(pid, sequence, attached.thread);
    }

    private static void successfulHandoffReleasesAfterTail() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        int pid = start(launcher, 1);
        check(pid == 4100 && backend.prepares == 1 && backend.activates == 1);
        ApplicationProcessRegistry.AttachedApplication attached = attach(registry, pid, 1);
        launcher.onAttached(attached);
        check(backend.cancels == 0 && backend.forgets == 1);
    }

    private static void cancellationWinsAndPreservesFailure() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        backend.failActivate = true;
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        RuntimeException failure = expectFailure(
                () -> start(launcher, 2));
        check("activate failed".equals(failure.getMessage()));
        check(backend.cancels == 1 && backend.forgets == 1);
        try {
            registry.beginAttachment(4100, UID, binder(), 2);
            throw new AssertionError("cancelled service was attachable");
        } catch (IllegalStateException expected) {
            // The cancellation tombstone is retained by the registry.
        }
    }

    private static void reentrantAttachmentWinsActivationFailure() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        backend.duringActivate = () -> {
            check(backend.forgets == 0);
            launcher.onAttached(attach(registry, backend.lastPid, backend.lastSequence));
        };
        backend.failActivate = true;
        check(start(launcher, 3) == 4100);
        check(backend.cancels == 0 && backend.forgets == 1);
    }

    private static void reentrantDeathRetiresOnlyAfterActivationTail() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        backend.duringActivate = () -> {
            check(backend.forgets == 0);
            launcher.onProcessGone(attachAndRetire(
                    registry, backend.lastPid, backend.lastSequence));
        };
        backend.failActivate = true;
        RuntimeException failure = expectFailure(
                () -> start(launcher, 4));
        check("activate failed".equals(failure.getMessage()));
        check(backend.cancels == 0 && backend.forgets == 1);
    }

    private static void cancellationRetryRetainsExactCap() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        backend.failActivate = true;
        backend.failCancel = true;
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        expectFailure(() -> start(launcher, 5));
        check(backend.cancels == 1 && backend.forgets == 0);
        backend.failCancel = false;
        backend.failActivate = false;
        // A later launch is the production retry clock for the retained cap.
        int nextPid = start(launcher, 7);
        check(backend.cancels == 2 && backend.forgets == 1);
        launcher.onAttached(attach(registry, nextPid, 7));
        check(backend.forgets == 2);
    }

    private static void duplicateSequenceAndReservationFailureAreExact() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        int pid = start(launcher, 8);
        check(expectFailure(() -> start(launcher, 8))
                .getMessage().contains("sequence"));
        launcher.onAttached(attach(registry, pid, 8));

        ApplicationProcessRegistry blocked = new ApplicationProcessRegistry();
        blocked.reserveBoundServiceProcess(4100, PACKAGE, PROCESS, UID, 80);
        Backend rejectedBackend = new Backend();
        BoundServiceProcessLauncher rejected = launcher(blocked, rejectedBackend);
        expectFailure(() -> start(rejected, 9));
        check(rejectedBackend.aborts == 1
                && rejectedBackend.cancels == 0 && rejectedBackend.forgets == 0);

        Backend malformedBackend = new Backend();
        malformedBackend.invalidPreparation = true;
        BoundServiceProcessLauncher malformed = launcher(
                new ApplicationProcessRegistry(), malformedBackend);
        expectFailure(() -> start(malformed, 81));
        check(malformedBackend.aborts == 1
                && malformedBackend.cancels == 0 && malformedBackend.forgets == 0);
    }

    private static void forgetFailureRemainsRetryable() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        int pid = start(launcher, 10);
        backend.failForget = true;
        launcher.onAttached(attach(registry, pid, 10));
        check(backend.forgets == 1);
        backend.failForget = false;
        launcher.retryPendingCancellation();
        check(backend.forgets == 2);
    }

    private static void backendForgetCanReenterOnAnotherThread() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        int pid = start(launcher, 11);
        backend.duringForget = () -> {
            Thread retry = new Thread(launcher::retryPendingCancellation);
            retry.start();
            try {
                retry.join(1000);
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                throw new AssertionError(interrupted);
            }
            check(!retry.isAlive());
        };
        launcher.onAttached(attach(registry, pid, 11));
        check(backend.forgets == 1);
    }

    private static void metadataFailureDoesNotInvalidateRealHandoff() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        backend.failForget = true;
        backend.duringActivate = () -> {
            launcher.onAttached(attach(registry, backend.lastPid, backend.lastSequence));
            check(backend.forgets == 0);
        };
        check(start(launcher, 12) == 4100);
        // A sticky old cleanup failure must not block an unrelated launch.
        backend.duringActivate = null;
        check(start(launcher, 13) == 4101);
        check(backend.cancels == 0 && backend.forgets == 2);
        backend.failForget = false;
        launcher.retryPendingCancellation();
        check(backend.forgets == 3);
    }

    private static void staleFailureDoesNotCancelUnprovenCap() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        backend.failActivate = true;
        backend.duringActivate = () -> {
            registry.cancelBoundServiceProcess(backend.lastPid, backend.lastSequence);
            registry.reserveBoundServiceProcess(
                    backend.lastPid, PACKAGE, PROCESS, UID, backend.lastSequence + 1);
        };
        expectFailure(() -> start(launcher, 6));
        check(backend.cancels == 0 && backend.forgets == 0);
    }

    private static ProcessLaunchTransport.PreparedLaunch prepare(
            BoundServiceProcessLauncher launcher, long sequence) {
        return launcher.prepare(PACKAGE, PROCESS, UID, false, sequence);
    }

    private static int start(BoundServiceProcessLauncher launcher, long sequence) {
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, sequence);
        check(launcher.activate(prepared) != ProcessLaunchTransport.ActivationResult.CANCELLED);
        return prepared.pid();
    }

    private static void prepareReservesWithoutActivating() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 20);
        check(prepared.pid() == 4100 && prepared.startSequence() == 20);
        check(backend.prepares == 1 && backend.activates == 0);
        launcher.retireUnattached(prepared);
        check(backend.cancels == 1 && backend.forgets == 1
                && backend.aborts == 0 && backend.activates == 0);
    }

    private static void retirementForbidsLaterActivation() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 21);
        launcher.retireUnattached(prepared);
        check(launcher.activate(prepared) == ProcessLaunchTransport.ActivationResult.CANCELLED);
        RuntimeException failure = expectFailure(() -> launcher.activate(prepared));
        check(failure instanceof IllegalStateException);
        check(backend.activates == 0 && backend.cancels == 1 && backend.forgets == 1
                && backend.aborts == 0);
    }

    private static void attachWinsRetirementWithoutKill() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 22);
        launcher.onAttached(attach(registry, prepared.pid(), prepared.startSequence()));
        check(backend.forgets == 1 && backend.cancels == 0 && backend.aborts == 0);
        launcher.retireUnattached(prepared);
        check(backend.cancels == 0 && backend.aborts == 0);
    }

    private static void foreignHandleIsRejected() {
        ApplicationProcessRegistry firstRegistry = new ApplicationProcessRegistry();
        Backend firstBackend = new Backend();
        BoundServiceProcessLauncher first = launcher(firstRegistry, firstBackend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(first, 23);

        ApplicationProcessRegistry secondRegistry = new ApplicationProcessRegistry();
        Backend secondBackend = new Backend();
        BoundServiceProcessLauncher second = launcher(secondRegistry, secondBackend);
        check(expectFailure(() -> second.activate(prepared)) instanceof IllegalArgumentException);
        check(expectFailure(() -> second.retireUnattached(prepared))
                instanceof IllegalArgumentException);
        first.retireUnattached(prepared);
        check(firstBackend.cancels == 1 && firstBackend.forgets == 1
                && firstBackend.aborts == 0 && secondBackend.activates == 0
                && secondBackend.aborts == 0);
    }

    private static void duplicateActivationIsRejected() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 24);
        check(launcher.activate(prepared) == ProcessLaunchTransport.ActivationResult.ACTIVE);
        RuntimeException failure = expectFailure(() -> launcher.activate(prepared));
        check(failure instanceof IllegalStateException);
        check(backend.activates == 1);
        launcher.onAttached(attach(registry, prepared.pid(), prepared.startSequence()));
        check(backend.forgets == 1);
    }

    private static void concurrentRetirementWaitsForActivationTail() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        backend.activateEntered = new CountDownLatch(1);
        backend.releaseActivate = new CountDownLatch(1);
        RuntimeException activationFailure = new IllegalStateException("activation tail failure");
        backend.activationFailure = activationFailure;
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 25);
        Throwable[] activationError = new Throwable[1];
        Throwable[] retirementError = new Throwable[1];
        Thread activation = new Thread(() -> {
            try {
                launcher.activate(prepared);
            } catch (Throwable failure) {
                activationError[0] = failure;
            }
        });
        activation.start();
        await(backend.activateEntered);

        Thread retirement = new Thread(() -> {
            try {
                launcher.retireUnattached(prepared);
            } catch (Throwable failure) {
                retirementError[0] = failure;
            }
        });
        retirement.start();
        try {
            retirement.join(100);
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError(interrupted);
        }
        // Retirement may finish its registry claim before activation returns,
        // but cleanup must remain behind the real activation tail.
        check(backend.forgets == 0);

        backend.releaseActivate.countDown();
        checkNoThrow(activation);
        checkNoThrow(retirement);
        checkSame(activationFailure, activationError[0]);
        check(retirementError[0] == null);
        check(backend.cancels == 1 && backend.forgets == 1 && backend.aborts == 0);
    }

    private static void cleanupFailuresRetainPrimaryAndRetryExactTail() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        RuntimeException activationFailure = new IllegalStateException("original activation");
        RuntimeException cancelFailure = new IllegalStateException("original cancellation");
        backend.activationFailure = activationFailure;
        backend.cancelFailure = cancelFailure;
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 26);
        RuntimeException observed = expectFailure(() -> launcher.activate(prepared));
        checkSame(activationFailure, observed);
        checkSame(cancelFailure, backend.observedCancelFailure);
        check(backend.cancels == 1 && backend.forgets == 0);
        backend.cancelFailure = null;
        launcher.retryPendingCancellation();
        check(backend.cancels == 2 && backend.forgets == 1);

        ApplicationProcessRegistry forgetRegistry = new ApplicationProcessRegistry();
        Backend forgetBackend = new Backend();
        BoundServiceProcessLauncher forgetLauncher = launcher(forgetRegistry, forgetBackend);
        ProcessLaunchTransport.PreparedLaunch attached = prepare(forgetLauncher, 27);
        RuntimeException forgetFailure = new IllegalStateException("original forget");
        forgetBackend.forgetFailure = forgetFailure;
        forgetLauncher.onAttached(attach(forgetRegistry, attached.pid(), attached.startSequence()));
        checkSame(forgetFailure, forgetBackend.observedForgetFailure);
        check(forgetBackend.forgets == 1);
        forgetBackend.forgetFailure = null;
        forgetLauncher.retryPendingCancellation();
        check(forgetBackend.forgets == 2);
    }

    private static void successfulReentrantHandoffSurvivesMetadataRemoval() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 30);
        backend.duringActivate = () -> launcher.onAttached(
                attach(registry, prepared.pid(), prepared.startSequence()));
        check(launcher.activate(prepared) == ProcessLaunchTransport.ActivationResult.HANDOFF);
        launcher.retireUnattached(prepared);
        check(backend.activates == 1 && backend.forgets == 1 && backend.cancels == 0);
    }

    private static void repeatedRetirementCannotReopenInFlightForget() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 31);
        backend.duringForget = () -> {
            launcher.retireUnattached(prepared);
            check(backend.cancels == 1 && backend.forgets == 1);
        };
        launcher.retireUnattached(prepared);
        launcher.retireUnattached(prepared);
        check(backend.cancels == 1 && backend.forgets == 1 && backend.activates == 0);
    }

    private static void cancellationTransportPinsPastActivationCompletion() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        Backend backend = new Backend();
        backend.activateEntered = new CountDownLatch(1);
        backend.releaseActivate = new CountDownLatch(1);
        backend.cancelEntered = new CountDownLatch(1);
        backend.releaseCancel = new CountDownLatch(1);
        BoundServiceProcessLauncher launcher = launcher(registry, backend);
        ProcessLaunchTransport.PreparedLaunch prepared = prepare(launcher, 32);
        Throwable[] errors = new Throwable[2];
        ProcessLaunchTransport.ActivationResult[] result = new ProcessLaunchTransport.ActivationResult[1];
        Thread activation = new Thread(() -> {
            try { result[0] = launcher.activate(prepared); }
            catch (Throwable error) { errors[0] = error; }
        });
        Thread retirement = new Thread(() -> {
            try { launcher.retireUnattached(prepared); }
            catch (Throwable error) { errors[1] = error; }
        });
        activation.start();
        await(backend.activateEntered);
        retirement.start();
        await(backend.cancelEntered);
        backend.releaseActivate.countDown();
        checkNoThrow(activation);
        check(errors[0] == null && result[0] == ProcessLaunchTransport.ActivationResult.CANCELLED);
        check(backend.forgets == 0);
        backend.releaseCancel.countDown();
        checkNoThrow(retirement);
        check(errors[1] == null && backend.cancels == 1 && backend.forgets == 1);
    }

    public static void main(String[] args) {
        successfulHandoffReleasesAfterTail();
        cancellationWinsAndPreservesFailure();
        reentrantAttachmentWinsActivationFailure();
        reentrantDeathRetiresOnlyAfterActivationTail();
        cancellationRetryRetainsExactCap();
        duplicateSequenceAndReservationFailureAreExact();
        forgetFailureRemainsRetryable();
        backendForgetCanReenterOnAnotherThread();
        metadataFailureDoesNotInvalidateRealHandoff();
        staleFailureDoesNotCancelUnprovenCap();
        prepareReservesWithoutActivating();
        retirementForbidsLaterActivation();
        attachWinsRetirementWithoutKill();
        foreignHandleIsRejected();
        duplicateActivationIsRejected();
        concurrentRetirementWaitsForActivationTail();
        cleanupFailuresRetainPrimaryAndRetryExactTail();
        successfulReentrantHandoffSurvivesMetadataRemoval();
        repeatedRetirementCannotReopenInFlightForget();
        cancellationTransportPinsPastActivationCompletion();
        System.out.println("BoundServiceProcessLauncher contract PASS");
    }
}
