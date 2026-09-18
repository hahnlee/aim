package dev.darwinart.runtime.wm;

import android.os.IBinder;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

public final class WindowSessionWindowOwnershipTest {
    private static final long TIMEOUT_SECONDS = 2;

    private static final class BinderKey implements InvocationHandler {
        final Object canonical = new Object();

        @Override public Object invoke(Object proxy, Method method, Object[] arguments) {
            if (method.getName().equals("equals")) {
                if (arguments == null || arguments.length != 1
                        || !(arguments[0] instanceof IBinder)) return false;
                BinderKey other = (BinderKey) Proxy.getInvocationHandler(arguments[0]);
                return canonical == other.canonical;
            }
            if (method.getName().equals("hashCode")) return System.identityHashCode(canonical);
            if (method.getReturnType() == boolean.class) return false;
            if (method.getReturnType() == int.class) return 0;
            return null;
        }
    }

    private static IBinder binder(BinderKey key) {
        return (IBinder) Proxy.newProxyInstance(
                WindowSessionWindowOwnershipTest.class.getClassLoader(),
                new Class<?>[] {IBinder.class}, key);
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void expectFailure(Runnable operation, String message) {
        try {
            operation.run();
            throw new AssertionError(message);
        } catch (IllegalArgumentException | IllegalStateException | SecurityException expected) {
            // Exact owner/record checks fail closed.
        }
    }

    private static void awaitWaiting(Thread thread, CountDownLatch started,
            AtomicReference<Throwable> failure) throws Exception {
        check(started.await(TIMEOUT_SECONDS, TimeUnit.SECONDS), "waiter did not start");
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(TIMEOUT_SECONDS);
        while (thread.getState() != Thread.State.WAITING && System.nanoTime() < deadline) {
            if (!thread.isAlive()) break;
            Thread.yield();
        }
        if (failure.get() != null) throw new AssertionError("waiter failed early", failure.get());
        check(thread.getState() == Thread.State.WAITING,
                "begin did not block in Object.wait: " + thread.getState());
    }

    private static void join(Thread thread, CountDownLatch done,
            AtomicReference<Throwable> failure) throws Exception {
        check(done.await(TIMEOUT_SECONDS, TimeUnit.SECONDS), "worker did not finish");
        thread.join(100);
        check(!thread.isAlive(), "worker remained alive");
        if (failure.get() != null) throw new AssertionError("worker failed", failure.get());
    }

    private static void testCanonicalAliasesAndAdmissionChecks() {
        WindowSessionWindowOwnership ownership = new WindowSessionWindowOwnership();
        Object session = new Object();
        IBinder first = binder(new BinderKey());
        IBinder alias = binder((BinderKey) Proxy.getInvocationHandler(first));
        WindowSessionWindowOwnership.Registration registration = ownership.claim(session, first);
        check(registration.window() == first, "canonical registration lost original Binder token");
        ownership.ready(session, registration);
        ownership.end(session, registration);
        check(ownership.require(session, alias) == registration,
                "Binder.equals/hash alias did not resolve canonical registration");
        expectFailure(() -> ownership.claim(session, alias), "duplicate canonical window accepted");
        expectFailure(() -> ownership.require(new Object(), first), "wrong session accepted");
        expectFailure(() -> ownership.require(session, binder(new BinderKey())),
                "unknown window accepted");
        expectFailure(() -> ownership.claim(null, binder(new BinderKey())),
                "null session accepted");
        expectFailure(() -> ownership.claim(session, null), "null window accepted");
        ownership.begin(session, first);
        ownership.retire(session, registration);
        ownership.release(session, registration);
        ownership.end(session, registration);
    }

    private static void testSameThreadAndStaleRelease() {
        WindowSessionWindowOwnership ownership = new WindowSessionWindowOwnership();
        Object session = new Object();
        IBinder window = binder(new BinderKey());
        WindowSessionWindowOwnership.Registration old = ownership.claim(session, window);
        expectFailure(() -> ownership.begin(session, window), "ADDING begin reentered");
        ownership.retire(session, old);
        ownership.release(session, old);
        ownership.end(session, old);
        expectFailure(() -> ownership.release(session, old), "stale release accepted");
        expectFailure(() -> ownership.end(session, old), "stale end accepted");

        WindowSessionWindowOwnership.Registration replacement = ownership.claim(session, window);
        ownership.ready(session, replacement);
        ownership.end(session, replacement);
        check(ownership.require(session, window) == replacement, "replacement was not admitted");
        expectFailure(() -> ownership.release(session, old), "old release removed replacement");
        expectFailure(() -> ownership.end(session, old), "old end changed replacement admission");
        check(ownership.require(session, window) == replacement,
                "stale operation altered replacement registration");
        ownership.begin(session, replacement.window());
        ownership.retire(session, replacement);
        ownership.release(session, replacement);
        ownership.end(session, replacement);
    }

    private static void testBlockedWaiterRejectsReleasedSuccessor() throws Exception {
        WindowSessionWindowOwnership ownership = new WindowSessionWindowOwnership();
        Object session = new Object();
        IBinder originalWindow = binder(new BinderKey());
        IBinder differentWindow = binder(new BinderKey());
        WindowSessionWindowOwnership.Registration original =
                ownership.claim(session, originalWindow);
        ownership.ready(session, original);
        ownership.end(session, original);
        ownership.begin(session, original.window());
        CountDownLatch started = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(1);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread waiter = new Thread(() -> {
            started.countDown();
            try {
                ownership.begin(session, originalWindow);
                failure.set(new AssertionError("released original waiter adopted successor"));
            } catch (SecurityException expected) {
                // The original record was released; a waiter cannot adopt a successor.
            } catch (Throwable error) {
                failure.set(error);
            } finally {
                done.countDown();
            }
        });
        waiter.start();
        awaitWaiting(waiter, started, failure);

        // A different window is independent while the original admission is blocked.
        WindowSessionWindowOwnership.Registration other = ownership.claim(session, differentWindow);
        ownership.ready(session, other);
        ownership.end(session, other);
        ownership.begin(session, differentWindow);
        ownership.retire(session, other);
        ownership.release(session, other);
        ownership.end(session, other);
        ownership.retire(session, original);
        ownership.release(session, original);
        WindowSessionWindowOwnership.Registration successor =
                ownership.claim(session, originalWindow);
        ownership.end(session, original);
        join(waiter, done, failure);

        // The old waiter must not adopt the replacement registration.
        check(ownership.require(session, originalWindow) == successor,
                "waiter changed to successor registration");
        ownership.ready(session, successor);
        ownership.end(session, successor);
        ownership.begin(session, successor.window());
        ownership.retire(session, successor);
        ownership.release(session, successor);
        ownership.end(session, successor);
    }

    private static void testInterruptedWaitPreservesInterrupt() throws Exception {
        WindowSessionWindowOwnership ownership = new WindowSessionWindowOwnership();
        Object session = new Object();
        IBinder window = binder(new BinderKey());
        WindowSessionWindowOwnership.Registration registration = ownership.claim(session, window);
        ownership.ready(session, registration);
        ownership.end(session, registration);
        ownership.begin(session, window);
        CountDownLatch started = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(1);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        Thread waiter = new Thread(() -> {
            started.countDown();
            try {
                ownership.begin(session, window);
                failure.set(new AssertionError("interrupted begin succeeded"));
            } catch (IllegalStateException expected) {
                if (!Thread.currentThread().isInterrupted())
                    failure.set(new AssertionError("interrupt status was not restored"));
            } catch (Throwable error) {
                failure.set(error);
            } finally {
                done.countDown();
            }
        });
        waiter.start();
        awaitWaiting(waiter, started, failure);
        waiter.interrupt();
        join(waiter, done, failure);
        ownership.retire(session, registration);
        ownership.release(session, registration);
        ownership.end(session, registration);
    }

    private static void testFinallyEndReleasesAdmission() {
        WindowSessionWindowOwnership ownership = new WindowSessionWindowOwnership();
        Object session = new Object();
        IBinder window = binder(new BinderKey());
        WindowSessionWindowOwnership.Registration registration = ownership.claim(session, window);
        try {
            throw new RuntimeException("simulated add failure");
        } catch (RuntimeException expected) {
            // The owner must always settle the exact admitted record.
        } finally {
            ownership.retire(session, registration);
            ownership.release(session, registration);
            ownership.end(session, registration);
        }
        WindowSessionWindowOwnership.Registration admitted = ownership.claim(session, window);
        check(admitted != registration, "failed add reused retired registration object");
        ownership.ready(session, admitted);
        ownership.end(session, admitted);
        ownership.begin(session, window);
        ownership.retire(session, admitted);
        ownership.release(session, admitted);
        ownership.end(session, admitted);
    }

    private static void testRetiringCleanupPath() {
        WindowSessionWindowOwnership ownership = new WindowSessionWindowOwnership();
        Object session = new Object();
        IBinder window = binder(new BinderKey());
        WindowSessionWindowOwnership.Registration registration = ownership.claim(session, window);
        ownership.ready(session, registration);
        ownership.end(session, registration);
        ownership.begin(session, window);
        ownership.retire(session, registration);
        ownership.end(session, registration);
        expectFailure(() -> ownership.begin(session, window),
                "normal operation entered RETIRING registration");
        WindowSessionWindowOwnership.Registration cleanup =
                ownership.beginCleanup(session, window);
        check(cleanup == registration, "cleanup did not retain exact retiring record");
        ownership.release(session, cleanup);
        ownership.end(session, cleanup);
        WindowSessionWindowOwnership.Registration replacement = ownership.claim(session, window);
        ownership.ready(session, replacement);
        ownership.end(session, replacement);
        ownership.begin(session, window);
        ownership.retire(session, replacement);
        ownership.release(session, replacement);
        ownership.end(session, replacement);
    }

    public static void main(String[] args) throws Exception {
        testCanonicalAliasesAndAdmissionChecks();
        testSameThreadAndStaleRelease();
        testBlockedWaiterRejectsReleasedSuccessor();
        testInterruptedWaitPreservesInterrupt();
        testFinallyEndReleasesAdmission();
        testRetiringCleanupPath();
        System.out.println("WMS canonical window ownership/admission lifecycle: PASS");
    }
}
