import android.os.IBinder;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.lang.reflect.Proxy;

public final class ApplicationProcessRegistryTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }

    private static IBinder binder() {
        return (IBinder) Proxy.newProxyInstance(
                ApplicationProcessRegistryTest.class.getClassLoader(),
                new Class<?>[] {IBinder.class},
                (proxy, method, arguments) -> {
                    // Binder handles have identity semantics. A proxy that
                    // returns false for equals() makes an otherwise exact
                    // process-incarnation check look like a forged caller.
                    if (method.getName().equals("equals")) {
                        return arguments != null && arguments.length == 1
                                && proxy == arguments[0];
                    }
                    if (method.getName().equals("hashCode")) {
                        return System.identityHashCode(proxy);
                    }
                    Class<?> result = method.getReturnType();
                    if (result == boolean.class) return false;
                    if (result == int.class) return 0;
                    return null;
                });
    }

    private static void expectFailure(Runnable operation) {
        try {
            operation.run();
            throw new AssertionError("operation unexpectedly succeeded");
        } catch (IllegalStateException | SecurityException expected) {
            // Expected fail-closed result.
        }
    }

    public static void main(String[] args) {
        callerMayUseIdentifiedProcessBeforeFinish();
        exactIncarnationDeathAndAbort();

        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder main = binder();
        registry.beginAttachment(41, 10000, main, 0);
        registry.identify(41, 0, main, "org.example.app");
        ApplicationProcessRegistry.AttachedApplication attached =
                registry.finishAttachment(41, 0);
        check(attached.thread == main);
        check(attached.packageName.equals("org.example.app"));
        check(attached.processName.equals("org.example.app"));
        check(attached.uid == 10000);
        check(attached.pid == 41);
        check(attached.startSequence == 0);
        check(attached.initialWork == ApplicationProcessRegistry.InitialWork.ACTIVITY);

        registry.reserveBoundServiceProcess(
                42, "org.example.app", "org.example.app:renderer", 99001, 7);
        // The child can call PM while its attachment handshake is still in
        // progress, so the reservation itself must establish ownership.
        check(registry.isCallerSameApp(99001, "org.example.app"));
        expectFailure(() -> registry.beginAttachment(42, 99001, binder(), 6));
        expectFailure(() -> registry.beginAttachment(42, 99002, binder(), 7));
        IBinder service = binder();
        registry.beginAttachment(42, 99001, service, 7);
        expectFailure(() -> registry.identify(42, 7, service, "org.example.other"));
        registry.identify(42, 7, service, "org.example.app");
        ApplicationProcessRegistry.AttachedApplication serviceAttached =
                registry.finishAttachment(42, 7);
        check(serviceAttached.thread == service);
        check(serviceAttached.packageName.equals("org.example.app"));
        check(serviceAttached.processName.equals("org.example.app:renderer"));
        check(serviceAttached.uid == 99001);
        check(serviceAttached.pid == 42);
        check(serviceAttached.startSequence == 7);
        check(serviceAttached.initialWork
                == ApplicationProcessRegistry.InitialWork.BOUND_SERVICE);
        check(registry.isCallerSameApp(99001, "org.example.app"));
        check(!registry.isCallerSameApp(99001, "org.example.other"));
        check(!registry.isCallerSameApp(10000, "org.example.app"));
        expectFailure(() -> registry.finishAttachment(42, 7));
        expectFailure(() -> registry.reserveBoundServiceProcess(
                42, "org.example.app", "org.example.app:other", 99002, 8));

        // A recycled isolated UID must not retain ownership of its old package.
        registry.reserveBoundServiceProcess(
                44, "org.example.other", "org.example.other:renderer", 99001, 10);
        check(registry.isCallerSameApp(99001, "org.example.other"));
        check(!registry.isCallerSameApp(99001, "org.example.app"));

        registry.reserveBoundServiceProcess(
                43, "org.example.app", "org.example.app:spare", 99002, 8);
        expectFailure(() -> registry.cancelBoundServiceProcess(43, 7));
        registry.cancelBoundServiceProcess(43, 8);
        expectFailure(() -> registry.beginAttachment(43, 99002, binder(), 8));
        expectFailure(() -> registry.reserveBoundServiceProcess(
                43, "org.example.app", "org.example.app:spare", 99002, 8));
        registry.reserveBoundServiceProcess(
                43, "org.example.app", "org.example.app:replacement", 99003, 9);
        registry.beginAttachment(43, 99003, binder(), 9);
        // A queued old attachment cannot claim the replacement's PID.
        expectFailure(() -> registry.beginAttachment(43, 99002, binder(), 8));
        expectFailure(() -> registry.cancelBoundServiceProcess(43, 9));
        check(registry.claimBoundServiceCancellation(43, 8).kind
                == ApplicationProcessRegistry.CancellationKind.STALE_OR_GONE);
        ApplicationProcessRegistry.BoundServiceCancellation claimed =
                registry.claimBoundServiceCancellation(43, 9);
        check(claimed.kind == ApplicationProcessRegistry.CancellationKind.ATTACHMENT_OWNED);
        check(claimed.attachment.pid == 43 && claimed.attachment.startSequence == 9);
        registry.reserveBoundServiceProcess(
                47, "org.example.app", "org.example.app:pending", 99004, 12);
        check(registry.claimBoundServiceCancellation(47, 12).kind
                == ApplicationProcessRegistry.CancellationKind.CANCELLED);
        expectFailure(() -> registry.beginAttachment(47, 99004, binder(), 12));
        System.out.println("ApplicationProcessRegistry launch ownership PASS");
    }

    private static void callerMayUseIdentifiedProcessBeforeFinish() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder thread = binder();
        registry.beginAttachment(45, 10045, thread, 11);
        // A thread is not an authorized caller until native-authenticated
        // package identity has been published.
        expectFailure(() -> registry.requireCaller(45, thread, "org.example.app"));
        registry.identify(45, 11, thread, "org.example.app");

        // Application.attachBaseContext runs during bindApplication, before
        // finishAttachApplication, and may call ATMS/AMS APIs.
        check(registry.requireCaller(45, thread, "org.example.app").thread == thread);
        expectFailure(() -> registry.requireCaller(45, binder(), "org.example.app"));
        expectFailure(() -> registry.requireCaller(45, thread, "org.example.other"));
        registry.finishAttachment(45, 11);
        expectFailure(() -> registry.finishAttachment(45, 11));
    }

    private static void exactIncarnationDeathAndAbort() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder oldThread = binder();
        registry.beginAttachment(46, 10046, oldThread, 21);
        IBinder wrongThread = binder();
        expectFailure(() -> registry.identify(46, 21, wrongThread, "org.example.app"));
        registry.identify(46, 21, oldThread, "org.example.app");
        registry.abortAttachment(46, 21, wrongThread);
        check(registry.requireCaller(46, oldThread, "org.example.app") != null);

        // A stale abort must not roll back the live incarnation.
        registry.abortAttachment(46, 20, oldThread);
        check(registry.requireCaller(46, oldThread, "org.example.app") != null);

        // Death during bind retires an identified, unfinished incarnation.
        check(registry.retireAttached(46, 21, oldThread) != null);
        check(registry.retireAttached(46, 21, oldThread) == null);

        // Reusing the PID is safe: the old death/abort keys cannot remove it.
        IBinder replacementThread = binder();
        registry.beginAttachment(46, 10046, replacementThread, 22);
        registry.identify(46, 22, replacementThread, "org.example.app");
        // The old incarnation's exact key must not abort a replacement while
        // its own bind is still in progress.
        registry.abortAttachment(46, 21, oldThread);
        check(registry.requireCaller(46, replacementThread, "org.example.app") != null);
        ApplicationProcessRegistry.AttachedApplication replacement =
                registry.finishAttachment(46, 22);
        check(registry.retireAttached(46, 21, oldThread) == null);
        check(registry.requireAttachedProcess(46).thread == replacementThread);
        ApplicationProcessRegistry.AttachedApplication retired =
                registry.retireAttached(46, 22, replacementThread);
        check(retired != null && retired.startSequence == replacement.startSequence
                && retired.thread == replacement.thread);
        check(registry.retireAttached(46, 22, replacementThread) == null);

        // An exact abort is the only rollback for a failed bind.
        IBinder failedThread = binder();
        registry.beginAttachment(46, 10046, failedThread, 23);
        registry.identify(46, 23, failedThread, "org.example.app");
        registry.abortAttachment(46, 23, failedThread);
        expectFailure(() -> registry.requireCaller(46, failedThread, "org.example.app"));
    }
}
