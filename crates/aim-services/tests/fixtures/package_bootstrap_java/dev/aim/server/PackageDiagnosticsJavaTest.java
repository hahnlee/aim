package dev.aim.server;

import android.app.admin.SecurityLog;
import android.content.pm.*;
import android.os.Binder;
import com.android.server.LocalServices;
import java.lang.reflect.Proxy;
import java.util.*;

final class PackageDiagnosticsJavaTest {
    @SuppressWarnings("unchecked")
    private static <T> T owner(Class<T> type, java.lang.reflect.InvocationHandler calls) {
        return (T) Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, calls);
    }
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
    static void run(String directory) throws Exception {
        var bridge = new PackageBootstrapBridge();
        var diagnostic = (PackageDiagnosticInputs) bridge.getPackageDiagnosticInputs();
        for (int uid : new int[] {0, 2000, 10100}) {
            Binder.uid = uid;
            for (Runnable action : new Runnable[] {() -> bridge.getPackageDiagnosticInputs(),
                    () -> bridge.logPackageProcessStart("p", "process", 10123, "default", "base.apk", 42),
                    () -> diagnostic.computeRolePackageStateHash(0), () -> diagnostic.dumpDexopt(null)}) {
                try { action.run(); throw new AssertionError("system UID guard missing"); }
                catch (SecurityException expected) {}
            }
        }
        Binder.uid = 1000;
        // Disabled logging needs no checksum owner. Enabled logging uses real APK bytes.
        bridge.logPackageProcessStart("p", "process", 10123, "default", "base.apk", 42);
        check(SecurityLog.events.isEmpty());
        var apk = java.nio.file.Path.of(directory, "base.apk");
        java.nio.file.Files.writeString(apk, "first APK contents");
        int[] requests = {0}; boolean[] dead = {false};
        var packages = new PackageLocalOwners.Packages(owner(IPackageManager.class, (proxy, method, args) -> {
            if (dead[0]) throw new android.os.RemoteException("package owner died");
            check(method.getName().equals("requestPackageChecksums"));
            check(args[0].equals("p") && !(boolean) args[1] && (int) args[2] == 0
                    && (int) args[3] == 8 && args[4] == null && (int) args[6] == 0);
            requests[0]++;
            var hash = java.security.MessageDigest.getInstance("SHA-256").digest(java.nio.file.Files.readAllBytes(apk));
            ((IOnChecksumsReadyListener) args[5]).onChecksumsReady(List.of(new ApkChecksum(null, 8, hash)));
            return null;
        }), () -> { throw new AssertionError("scan not attached"); });
        LocalServices.addService(PackageManagerInternal.class, packages);
        SecurityLog.enabled = true;
        var pending = new ArrayDeque<Runnable>();
        android.os.Handler.dispatch = pending::add;
        for (int i = 0; i < 2; i++) bridge.logPackageProcessStart("p", "process", 10123, "default", apk.toString(), 42);
        check(requests[0] == 1 && SecurityLog.events.isEmpty());
        while (!pending.isEmpty()) pending.remove().run();
        String hash = java.util.HexFormat.of().formatHex(java.security.MessageDigest.getInstance("SHA-256").digest(java.nio.file.Files.readAllBytes(apk)));
        check(SecurityLog.events.size() == 2);
        var event = SecurityLog.events.get(0);
        check(event[0].equals("process") && (long) event[1] > 0 && event[2].equals(10123)
                && event[3].equals(42) && event[4].equals("default") && event[5].equals(hash));
        java.nio.file.Files.writeString(apk, "replacement APK contents");
        com.android.server.pm.NativeProcessLoggingBridge.invalidate(apk.toString());
        bridge.logPackageProcessStart("p", "process", 10123, "default", apk.toString(), 42);
        while (!pending.isEmpty()) pending.remove().run();
        check(requests[0] == 2 && !SecurityLog.events.get(2)[5].equals(hash));
        bridge.logPackageProcessStart("p", "process", 10123, "default", null, 42);
        while (!pending.isEmpty()) pending.remove().run();
        check(SecurityLog.events.get(3)[5].equals("No APK"));
        dead[0] = true;
        try { packages.requestChecksums("p", false, 0, 8, null, sums -> {}, 0, Runnable::run, new android.os.Handler()); throw new AssertionError(); }
        catch (RuntimeException expected) { check(expected.getMessage().contains("package owner died")); }
        android.os.Handler.dispatch = Runnable::run;
        var state = new PackageUserStateData(); state.on = new String[] {"p.Enabled"};
        int[] captures = {0};
        var endpoint = owner(IPackageScanSnapshot.class, (proxy, method, args) -> {
            if (method.getName().equals("getPackageNames")) return new String[] {"p"};
            if (method.getName().equals("close")) return null;
            throw new AssertionError(method);
        });
        PackageScanLease.states.put(endpoint, Map.of("p", state));
        packages = new PackageLocalOwners.Packages(nullPackageOwner(), () -> { captures[0]++; return endpoint; });
        LocalServices.addService(PackageManagerInternal.class, packages);
        LocalServices.addService(com.android.server.pm.UserManagerInternal.class,
                new PackageLocalOwners.Users(owner(android.os.IUserManager.class, (p,m,a) -> {
                    check(m.getName().equals("getUserInfo"));
                    return (int) a[0] == 0 ? new UserInfo(0) : null;
                })));
        String first = diagnostic.computeRolePackageStateHash(0);
        check(captures[0] == 1 && PackageScanLease.closed == 1);
        state.enabled = 2; state.off = new String[] {"p.Disabled"};
        check(!first.equals(diagnostic.computeRolePackageStateHash(0)) && captures[0] == 2);
        state.installed = false;
        String empty = diagnostic.computeRolePackageStateHash(0);
        check(!empty.equals(first));
        for (int user : new int[] {-1, 10}) {
            try { diagnostic.computeRolePackageStateHash(user); throw new AssertionError(); }
            catch (IllegalArgumentException expected) {}
        }
        try { diagnostic.dumpDexopt(null); throw new AssertionError(); }
        catch (IllegalStateException expected) {}
        int[] closed = {0};
        var snapshot = owner(com.android.server.pm.PackageManagerLocal.FilteredSnapshot.class, (p,m,a) -> {
            check(m.getName().equals("close")); closed[0]++; return null;
        });
        com.android.server.LocalManagerRegistry.addManager(com.android.server.pm.PackageManagerLocal.class,
                owner(com.android.server.pm.PackageManagerLocal.class, (p,m,a) -> {
                    check(m.getName().equals("withFilteredSnapshot")); return snapshot;
                }));
        com.android.server.LocalManagerRegistry.addManager(com.android.server.art.ArtManagerLocal.class, new com.android.server.art.ArtManagerLocal());
        check(diagnostic.dumpDexopt(null).contains("all dexopt"));
        check(diagnostic.dumpDexopt("p").contains("dexopt p"));
        check(diagnostic.dumpDexopt("absent").contains("unknown package"));
        try { diagnostic.dumpDexopt("dead"); throw new AssertionError(); }
        catch (IllegalStateException expected) { check(expected.getMessage().equals("ART died")); }
        check(closed[0] == 4);
    }
    private static IPackageManager nullPackageOwner() {
        return owner(IPackageManager.class, (p,m,a) -> { throw new AssertionError(m); });
    }
}
