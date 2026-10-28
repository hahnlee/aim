package dev.aim.server;
import android.os.Binder;
import java.util.List;

public final class PackageShutdownJavaTest {
    private static final java.util.ArrayList<String> calls = new java.util.ArrayList<>();
    private static String failure;
    public static void step(String name) {
        calls.add(name);
        if (name.equals(failure)) throw new IllegalStateException(name);
    }
    @SuppressWarnings("unchecked")
    private static <T> T owner(Class<T> type, java.lang.reflect.InvocationHandler calls) {
        return (T) java.lang.reflect.Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, calls);
    }
    public static void run() {
        var p = new android.content.pm.PackageInfo(); p.packageName = "installed";
        android.os.IUserManager.Stub.service = owner(android.os.IUserManager.class, (proxy, method, args) -> {
            if (!method.getName().equals("getUsers") || !(boolean) args[0]
                    || (boolean) args[1] || !(boolean) args[2]) throw new AssertionError(method);
            return List.of(new android.content.pm.UserInfo(0), new android.content.pm.UserInfo(10));
        });
        android.content.pm.IPackageManager.Stub.service = owner(android.content.pm.IPackageManager.class, (proxy, method, args) -> {
            if (!method.getName().equals("getInstalledPackages") || (long) args[0] != 0) throw new AssertionError(method);
            return new android.content.pm.ParceledListSlice<>((int) args[1] == 0 ? List.of(p) : List.of());
        });
        var backend = owner(ISystemProcess.class, (proxy, method, args) -> {
            if (!method.getName().equals("writePackageCompilerStats")) throw new AssertionError(method);
            step("compiler"); return null;
        });
        var context = new android.content.Context();
        var installedShutdown = PackageLocalOwners.publish(backend, context, new com.android.server.pm.Installer(context));
        var packages = com.android.server.LocalServices.getService(android.content.pm.PackageManagerInternal.class);
        var actualDynamic = packages.getDynamicCodeLogger();
        var actualDex = com.android.server.pm.dex.DexManager.current;
        if (actualDex.installed != actualDynamic.installed || !actualDex.installed.keySet().equals(java.util.Set.of(0, 10))
                || actualDex.installed.get(0).get(0) != p || !actualDex.installed.get(10).isEmpty()) throw new AssertionError();
        try { installedShutdown.writeStatisticsNow(); }
        catch (android.os.RemoteException e) { throw new AssertionError(e); }
        if (!calls.equals(List.of("compiler", "dex", "dynamic", "watchdog"))) throw new AssertionError(calls);
        calls.clear();
        var dynamic = new com.android.server.pm.dex.DynamicCodeLogger(null);
        var dex = new com.android.server.pm.dex.DexManager(null, null, dynamic);
        var shutdown = new com.android.server.pm.NativePackageShutdownBridge(
                new android.content.Context(), () -> step("compiler"), dex, dynamic);
        var bridge = new PackageBootstrapBridge();
        bridge.installPackageShutdownOwner(shutdown);
        try { bridge.installPackageShutdownOwner(shutdown); throw new AssertionError(); }
        catch (IllegalStateException expected) {}
        if (bridge.getPackageShutdownBridge() != shutdown) throw new AssertionError();
        for (int uid : new int[] {0, 2000, 10100}) {
            Binder.uid = uid;
            try { shutdown.writeStatisticsNow(); throw new AssertionError(); }
            catch (SecurityException expected) {}
        }
        if (!calls.isEmpty()) throw new AssertionError(calls);
        Binder.uid = 1000;
        android.crashrecovery.flags.Flags.refactored = false;
        shutdown.writeStatisticsNow();
        if (!calls.equals(List.of("compiler", "dex", "dynamic", "watchdog"))) throw new AssertionError(calls);
        calls.clear();
        int instances = com.android.server.PackageWatchdog.instances;
        android.crashrecovery.flags.Flags.refactored = true;
        shutdown.writeStatisticsNow();
        if (!calls.equals(List.of("compiler", "dex", "dynamic"))
                || instances != com.android.server.PackageWatchdog.instances) throw new AssertionError(calls);
        for (String stop : List.of("compiler", "dex", "dynamic")) {
            calls.clear(); failure = stop;
            try { shutdown.writeStatisticsNow(); throw new AssertionError(); }
            catch (IllegalStateException expected) { if (!expected.getMessage().equals(stop)) throw expected; }
            var expected = List.of("compiler", "dex", "dynamic");
            if (!calls.equals(expected.subList(0, expected.indexOf(stop) + 1))) throw new AssertionError(calls);
        }
        failure = null;
        android.crashrecovery.flags.Flags.refactored = false;
    }
}
