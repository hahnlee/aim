package dev.aim.server;

import android.os.IBinder;
import android.os.UserHandle;
import com.android.server.pm.pkg.PackageState;
import java.lang.reflect.Proxy;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;

/** Real Store/Local/version mapping and startup factory; only Android/native boundaries doubled. */
public final class ArtPackageLocalTest {
    @SuppressWarnings("unchecked")
    private static <T> T proxy(Class<T> type, java.lang.reflect.InvocationHandler handler) {
        return (T) Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, handler);
    }
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
    private static void closed(Runnable action) {
        try { action.run(); throw new AssertionError("closed owner accepted"); }
        catch (IllegalStateException expected) {}
    }
    public static void run(String directory) throws Exception {
        for (int retirement : new int[] {0, 1, 2}) {
            int kills = android.os.Process.kills;
            Path path = Path.of(directory, "art-version-" + retirement);
            try (var writer = new java.io.RandomAccessFile(path.toFile(), "rw")) {
                publish(writer, 1);
                IBinder.DeathRecipient[] recipient = {null};
                int[] unlinks = {0}, captures = {0}, sdk = {0}, signing = {0};
                boolean[] alive = {true}, failCapture = {true};
                long[] epoch = {1}, latest = {1};
                var binder = proxy(IBinder.class, (p, m, a) -> switch (m.getName()) {
                    case "linkToDeath" -> { recipient[0] = (IBinder.DeathRecipient) a[0]; yield null; }
                    case "unlinkToDeath" -> { check(recipient[0] == a[0]); unlinks[0]++; yield true; }
                    default -> throw new AssertionError(m);
                });
                var backend = proxy(IDexUsePackages.class, (p, m, a) -> {
                    if (!alive[0] || epoch[0] != 1) throw new IllegalStateException("native epoch ended");
                    return switch (m.getName()) {
                        case "crossUserSuspensions" -> true;
                        case "getSnapshot" -> {
                            if (failCapture[0]) throw new android.os.RemoteException("source unavailable");
                            captures[0]++;
                            long version = latest[0];
                            PackageState state = proxy(PackageState.class, (s, getter, args) -> {
                                if (getter.getName().equals("getPackageName")) return "p";
                                if (getter.getName().equals("getVersionCode")) return version;
                                throw new AssertionError(getter);
                            });
                            var endpoint = proxy(IPackageScanSnapshot.class, (e, method, args) -> {
                                if (method.getName().equals("getVersion")) return version;
                                if (method.getName().equals("close")) return null;
                                throw new AssertionError(method);
                            });
                            PackageScanLease.captures.put(endpoint, Map.of("p", state));
                            yield endpoint;
                        }
                        case "filterPackageName" -> {
                            check((long) a[0] <= latest[0]);
                            yield (int) a[3] == 0 && (int) a[2] == 10100
                                    && (a[1].equals("p") || a[1].equals("alias")) ? "p" : null;
                        }
                        case "shouldFilter" -> (int) a[3] != 0 || (int) a[2] != 10100;
                        default -> throw new AssertionError(m);
                    };
                });
                var host = proxy(IServiceHost.class, (p, m, a) -> switch (m.getName()) {
                    case "getPackageStateVersionPage" -> android.os.ParcelFileDescriptor.open(
                            path.toFile(), android.os.ParcelFileDescriptor.MODE_READ_ONLY);
                    case "reconcilePackageSdkData" -> {
                        check(java.util.Arrays.equals(a, new Object[] {"vol", "p", java.util.List.of("sdk"), 0, 10100, -1, "se", 3}));
                        sdk[0]++; yield null;
                    }
                    case "clearPackageSigningOverrides" -> { signing[0]++; yield 1L; }
                    default -> throw new AssertionError(m);
                });
                try { PackageBootstrapBridge.captureLocal(host, binder, backend); throw new AssertionError("missing source accepted"); }
                catch (android.os.RemoteException expected) { check(expected.getMessage().equals("source unavailable")); }
                check(unlinks[0] == 1 && android.os.Process.kills == kills);
                failCapture[0] = false; unlinks[0] = 0;
                var local = PackageBootstrapBridge.captureLocal(host, binder, backend);
                check(captures[0] == 1);
                try (var old = local.withUnfilteredSnapshot(); var child = old.filtered(10100, UserHandle.of(0))) {
                    PackageState first = child.getPackageState("alias");
                    check(first == old.getPackageStates().get("p"));
                    check(first.getVersionCode() == 1);
                    try (var hidden = local.withFilteredSnapshot(10100, UserHandle.of(10));
                            var foreign = local.withFilteredSnapshot(10101, UserHandle.of(0))) {
                        check(hidden.getPackageState("alias") == null && hidden.getPackageStates().isEmpty());
                        check(foreign.getPackageState("p") == null && foreign.getPackageStates().isEmpty());
                    }
                    publish(writer, 2);
                    closed(local::withUnfilteredSnapshot); // A stale source cannot satisfy a newer page.
                    latest[0] = 2;
                    // No notifyDexLoad, explicit refresh or second package-state owner.
                    try (var next = local.withFilteredSnapshot(10100, UserHandle.of(0))) {
                        check(next.getPackageState("alias").getVersionCode() == 2);
                        check(next.getPackageState("p") != first);
                    }
                    check(captures[0] == 3 && child.getPackageState("alias") == first);
                    local.reconcileSdkData("vol", "p", java.util.List.of("sdk"), 0, 10100, -1, "se", 3);
                    local.clearOverrideSigningDetails();
                    check(sdk[0] == 1 && signing[0] == 1 && captures[0] == 3);
                    old.close();
                    closed(child::getPackageStates);
                    closed(() -> old.filtered(10100, UserHandle.of(0)).getPackageStates());
                    if (retirement == 0) local.closeEpoch();
                    else if (retirement == 1) { alive[0] = false; recipient[0].binderDied(); }
                    else { epoch[0]++; publish(writer, 0); closed(local::clearOverrideSigningDetails); }
                    check(android.os.Process.kills == kills + (retirement == 0 ? 0 : 1) && unlinks[0] == 1);
                    closed(local::withUnfilteredSnapshot);
                    closed(local::clearOverrideSigningDetails);
                    local.closeEpoch();
                    check(unlinks[0] == 1);
                }
            } finally { Files.deleteIfExists(path); }
        }
    }
    private static void publish(java.io.RandomAccessFile writer, long version) throws Exception {
        writer.seek(0);
        writer.write(ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(version).array());
        writer.getFD().sync();
    }
}
