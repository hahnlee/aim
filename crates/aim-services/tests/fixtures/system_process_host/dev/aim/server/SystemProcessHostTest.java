package dev.aim.server;
import java.util.ArrayList;
import java.util.List;
public final class SystemProcessHostTest {
    public static final List<String> events = new ArrayList<>();
    public static final android.os.IBinder applicationThread = new android.os.Binder();
    static final IForegroundServicePolicy foregroundPolicy = (IForegroundServicePolicy)
            java.lang.reflect.Proxy.newProxyInstance(SystemProcessHostTest.class.getClassLoader(),
                    new Class<?>[] {IForegroundServicePolicy.class},
                    (proxy, method, arguments) -> { throw new AssertionError(method); });
    public static final class Attached extends Error {}
    public static void main(String[] args) throws Exception {
        try {
            SystemProcessHost.Server.main(new String[0]);
            throw new AssertionError("startup returned");
        } catch (Attached expected) {
            if (!events.equals(List.of("context", "modules", "memory", "domain",
                    "attachDomain", "foreground", "attachSystem"))) {
                throw new AssertionError(events);
            }
        }
    }
}
