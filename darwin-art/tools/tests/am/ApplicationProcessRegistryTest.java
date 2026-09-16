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
        ApplicationProcessRegistry registry = new ApplicationProcessRegistry();
        IBinder main = binder();
        registry.beginAttachment(41, 10000, main, 0);
        registry.identify(41, "org.example.app");
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
        expectFailure(() -> registry.identify(42, "org.example.other"));
        registry.identify(42, "org.example.app");
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
        registry.reserveBoundServiceProcess(
                43, "org.example.app", "org.example.app:replacement", 99003, 9);
        registry.beginAttachment(43, 99003, binder(), 9);
        expectFailure(() -> registry.cancelBoundServiceProcess(43, 9));
        System.out.println("ApplicationProcessRegistry launch ownership PASS");
    }
}
