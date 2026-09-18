package dev.darwinart.runtime.wm;

import android.os.IBinder;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.lang.reflect.Proxy;

public final class WindowSessionIdentityTest {
    private static IBinder binder() {
        return (IBinder) Proxy.newProxyInstance(
                WindowSessionIdentityTest.class.getClassLoader(),
                new Class<?>[] {IBinder.class},
                (proxy, method, arguments) -> {
                    if (method.getName().equals("equals")) {
                        return arguments != null && arguments.length == 1
                                && proxy == arguments[0];
                    }
                    if (method.getName().equals("hashCode"))
                        return System.identityHashCode(proxy);
                    Class<?> result = method.getReturnType();
                    if (result == boolean.class) return false;
                    if (result == int.class) return 0;
                    return null;
                });
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void expectFailure(Runnable operation, String message) {
        try {
            operation.run();
            throw new AssertionError(message);
        } catch (IllegalStateException | SecurityException expected) {
            // Authentication failures must fail closed.
        }
    }

    private static void testIdentifiedBeforeFinishAndStableState() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder thread = binder();
        registry.beginAttachment(41, 10041, thread, 8);
        registry.identify(41, 8, thread, "org.example.app");
        WindowSessionIdentity session = new WindowSessionIdentity(registry, 41, 10041);
        check(session.pid() == 41, "session retains owner pid");
        session.requireCaller(41, 10041); // bindApplication is still in progress.
        ApplicationProcessRegistry.AttachedApplication beforeFinish =
                registry.requireIdentifiedAttachment(41, 10041);
        check(!beforeFinish.packageName.isEmpty() && beforeFinish.startSequence == 8,
                "identified attachment snapshot is complete before finish");
        registry.finishAttachment(41, 8);
        session.requireCaller(41, 10041);
        ApplicationProcessRegistry.AttachedApplication afterFinish =
                registry.requireIdentifiedAttachment(41, 10041);
        check(afterFinish.thread == thread && afterFinish.startSequence == 8,
                "finish does not change authenticated incarnation");

        expectFailure(() -> session.requireCaller(40, 10041), "wrong pid accepted");
        expectFailure(() -> session.requireCaller(41, 10042), "wrong uid accepted");
        expectFailure(() -> registry.requireIdentifiedAttachment(41, -1),
                "negative uid accepted");
        check(registry.requireIdentifiedAttachment(41, 10041).thread == thread,
                "failed authentication did not mutate live state");
    }

    private static void testUnidentifiedAndRetiredFail() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder thread = binder();
        registry.beginAttachment(42, 10042, thread, 12);
        expectFailure(() -> registry.requireIdentifiedAttachment(42, 10042),
                "unidentified attachment accepted");
        registry.identify(42, 12, thread, "org.example.app");
        WindowSessionIdentity session = new WindowSessionIdentity(registry, 42, 10042);
        check(registry.retireAttached(42, 12, thread) != null, "retirement failed");
        expectFailure(() -> session.requireCaller(42, 10042), "retired session accepted");
    }

    private static void testPidReuseAndIncarnationChecks() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder oldThread = binder();
        registry.beginAttachment(43, 10043, oldThread, 20);
        registry.identify(43, 20, oldThread, "org.example.app");
        WindowSessionIdentity oldSession = new WindowSessionIdentity(registry, 43, 10043);
        registry.finishAttachment(43, 20);
        registry.retireAttached(43, 20, oldThread);

        // Same PID, UID, and package is still a new process incarnation.
        IBinder replacementThread = binder();
        registry.beginAttachment(43, 10043, replacementThread, 21);
        registry.identify(43, 21, replacementThread, "org.example.app");
        expectFailure(() -> oldSession.requireCaller(43, 10043),
                "same-package PID reuse accepted");
        check(new WindowSessionIdentity(registry, 43, 10043).pid() == 43,
                "replacement session did not authenticate");
        expectFailure(() -> registry.requireIdentifiedAttachment(43, 10044),
                "replacement UID mismatch accepted");

        // Even reusing the old Binder handle cannot hide a changed start epoch.
        registry.retireAttached(43, 21, replacementThread);
        registry.beginAttachment(43, 10043, oldThread, 22);
        registry.identify(43, 22, oldThread, "org.example.app");
        expectFailure(() -> oldSession.requireCaller(43, 10043),
                "changed start sequence accepted");
    }

    private static void testInvalidOwnerArguments() {
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        expectFailure(() -> new WindowSessionIdentity(registry, 99, 10099),
                "missing process accepted");
        boolean threw = false;
        try { new WindowSessionIdentity(null, 1, 1); }
        catch (IllegalArgumentException expected) { threw = true; }
        check(threw, "null registry accepted");
    }

    public static void main(String[] args) {
        testIdentifiedBeforeFinishAndStableState();
        testUnidentifiedAndRetiredFail();
        testPidReuseAndIncarnationChecks();
        testInvalidOwnerArguments();
        System.out.println("WMS session identity snapshots and incarnation reauthentication: PASS");
    }
}
