package dev.aim.server;
import java.util.ArrayList;
import java.util.List;
public final class SystemProcessHostTest {
    public static boolean deviceLock;
    public static void loadLibrary(String name) {
        if (!name.equals("android_servers")) throw new AssertionError(name);
        events.add("jni");
    }
    public static final List<String> events = new ArrayList<>();
    public static final android.os.IBinder applicationThread = new android.os.Binder();
    static final IForegroundServicePolicy foregroundPolicy = (IForegroundServicePolicy)
            java.lang.reflect.Proxy.newProxyInstance(SystemProcessHostTest.class.getClassLoader(),
                    new Class<?>[] {IForegroundServicePolicy.class},
                    (proxy, method, arguments) -> { throw new AssertionError(method); });
    public static final class Attached extends Error {}
    public static void main(String[] args) throws Exception {
        deviceLock = Boolean.parseBoolean(args[0]);
        try {
            SystemProcessHost.Server.main(new String[0]);
            throw new AssertionError("startup returned");
        } catch (Attached expected) {
            List<String> wanted = new ArrayList<>(List.of("jni", "context", "modules", "memory",
                    "systemApplication", "network", "domain", "attachDomain", "foreground",
                    "attachSystem", "companion", "crates", "NetworkStatsInitializer:construct",
                    "NetworkStatsInitializer:start", "NearbyServiceInitializer:construct",
                    "NearbyServiceInitializer:start", "StatsPullAtomService:construct",
                    "StatsPullAtomService:start", "AdServices:construct", "AdServices:start"));
            List<String> early = List.of("NearbyServiceInitializer", "StatsPullAtomService", "AdServices");
            for (int phase : new int[] {480, 500, 520}) {
                for (String service : early) wanted.add(service+":phase="+phase);
            }
            wanted.addAll(List.of("AppSearchModule$Lifecycle:construct", "AppSearchModule$Lifecycle:start"));
            List<String> services = new ArrayList<>(early);
            services.add("AppSearchModule$Lifecycle");
            if (deviceLock) {
                wanted.addAll(List.of("DeviceLockService:construct", "DeviceLockService:start"));
                services.add("DeviceLockService");
            }
            for (int phase : new int[] {550, 600}) {
                for (String service : services) wanted.add(service+":phase="+phase);
            }
            for (String state : List.of("starting", "unlocking", "unlocked", "phase=1000")) {
                for (String service : services) wanted.add(service+":"+state);
            }
            wanted.add("telecom");
            if (!events.equals(wanted)) throw new AssertionError("actual="+events+" expected="+wanted);
        }
    }
}
