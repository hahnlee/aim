package dev.aim.server;

import android.os.Binder;
import com.android.server.LocalManagerRegistry;
import com.android.server.art.ArtManagerLocal;
import com.android.server.art.DexUseManagerLocal;
import com.android.server.pm.PackageManagerLocal;
import java.util.Map;

/** Production startup/deletion glue; original ART and native transport are boundary doubles. */
public final class ArtOwnerTest {
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
    @FunctionalInterface interface Action { void run() throws Exception; }
    private static void rejected(Class<? extends Throwable> type, Action action) throws Exception {
        try { action.run(); throw new AssertionError("accepted " + type); }
        catch (Exception failure) { if (!type.isInstance(failure)) throw failure; }
    }
    public static void main(String[] args) throws Exception {
        String mode = args[0];
        var context = new android.content.Context();
        var policy = new PackageSnapshots.Owner() {
            public String getFilteredPackageName(long v, String n, int uid, int user) {
                return uid == 1_010_100 && user == 10 && ("p".equals(n) || "alias".equals(n)) ? "p" : null;
            }
            public boolean shouldFilter(long v, com.android.server.pm.pkg.PackageState s, int uid, int user) {
                return uid != 1_010_100 || user != 10;
            }
        };
        var state = (com.android.server.pm.pkg.PackageState) java.lang.reflect.Proxy.newProxyInstance(
                ArtOwnerTest.class.getClassLoader(), new Class<?>[] {com.android.server.pm.pkg.PackageState.class},
                (p,m,a) -> { if (m.getName().equals("getPackageName")) return "p"; throw new AssertionError(m); });
        var local = new PackageLocal(new PackageSnapshots.Data(1, Map.of("p", state), Map.of(), Map.of(), policy),
                (v,n,d,u,a,p,s,f) -> { throw new AssertionError(); }, new PackageLocal.SigningOwner() {
                    public void add(android.content.pm.SigningDetails a, android.content.pm.SigningDetails b) { throw new AssertionError(); }
                    public void remove(android.content.pm.SigningDetails a) { throw new AssertionError(); }
                    public void clear() { throw new AssertionError(); }
                });
        IArtDeletion[] attached = {null}; boolean[] alive = {true}; int[] checks = {0};
        var epoch = (IDexUsePackages) java.lang.reflect.Proxy.newProxyInstance(ArtOwnerTest.class.getClassLoader(),
                new Class<?>[] {IDexUsePackages.class}, (p,m,a) -> switch (m.getName()) {
                    case "getArtOwnerGeneration" -> 7L;
                    case "attachArtOwner" -> {
                        check((long)a[0] == 7 && attached[0] == null);
                        if (mode.equals("attach")) throw new android.os.RemoteException("native attachment failed");
                        attached[0] = (IArtDeletion)a[1]; yield null;
                    }
                    case "checkArtOwner" -> { checks[0]++; check((long)a[0] == 7);
                        if (!alive[0]) throw new IllegalStateException("native epoch ended"); yield null; }
                    default -> throw new AssertionError(m);
                });
        if (!mode.equals("packages")) LocalManagerRegistry.addManager(PackageManagerLocal.class, local);
        var dex = new DexUseManagerLocal();
        if (!mode.equals("dex")) LocalManagerRegistry.addManager(DexUseManagerLocal.class, dex);
        ArtManagerLocal.missingDependency = mode.equals("dependency");
        if (!mode.equals("normal")) {
            rejected(mode.equals("attach") ? android.os.RemoteException.class : NullPointerException.class,
                    () -> ArtOwnerStartup.start(context, epoch));
            check(attached[0] == null);
            if (!mode.equals("packages")) rejected(IllegalStateException.class, local::withUnfilteredSnapshot);
            check((LocalManagerRegistry.getManager(ArtManagerLocal.class) != null) == mode.equals("attach"));
            return;
        }
        long start = System.nanoTime();
        ArtOwnerStartup.start(context, epoch);
        System.out.println("ART startup glue with doubled original constructor: " + (System.nanoTime()-start)/1000 + " us");
        var art = LocalManagerRegistry.getManager(ArtManagerLocal.class);
        check(art != null && ArtManagerLocal.constructions == 1 && ArtManagerLocal.calls == 0);
        check(LocalManagerRegistry.getManager(PackageManagerLocal.class) == local);
        check(LocalManagerRegistry.getManager(DexUseManagerLocal.class) == dex && LocalManagerRegistry.size() == 3);
        rejected(IllegalStateException.class, () -> NativePackageManagerService.OriginalArtOwner.construct(context, local));
        for (int uid : new int[] {0, 10100}) {
            Binder.uid = uid;
            rejected(SecurityException.class, () -> attached[0].clearAppProfiles(7, "p"));
            rejected(SecurityException.class, () -> attached[0].deleteDexoptArtifacts(7, "p", 10100, 10));
        }
        Binder.uid = 1000;
        check(checks[0] == 1 && ArtManagerLocal.calls == 0);
        rejected(IllegalStateException.class, () -> attached[0].clearAppProfiles(6, "p"));
        attached[0].clearAppProfiles(7, "p");
        check(attached[0].deleteDexoptArtifacts(7, "p", 10100, 10) == 1L << 40);
        rejected(IllegalArgumentException.class, () -> attached[0].clearAppProfiles(7, "error"));
        check(attached[0].deleteDexoptArtifacts(7, "error", 10100, 0) == -1);
        // WritableArtifactLists' three original result lists, including primary/split/secondary
        // ODEX/VDEX/ART, runtime images and SDM/SDC. This doubles ART's result, not its path walker.
        var artifacts = new java.util.LinkedHashMap<String, Long>();
        for (String container : new String[] {"base", "split", "secondary"})
            for (String kind : new String[] {"odex", "vdex", "art", "runtime", "sdm", "sdc"})
                artifacts.put(container + "." + kind, (1L << 33) + artifacts.size());
        var categories = Map.copyOf(artifacts);
        PackageManagerLocal.FilteredSnapshot[] last = {null};
        int[] effects = {0};
        ArtManagerLocal.deletion = (snapshot, name) -> {
            last[0] = snapshot;
            if (snapshot.getPackageState(name) == null) throw new IllegalArgumentException("unknown/hidden package");
            check(snapshot.getPackageState(name) == state && snapshot.getPackageStates().size() == 1
                    && snapshot.getPackageStates().get("p") == state);
            long bytes = artifacts.values().stream().mapToLong(Long::longValue).sum();
            effects[0] += artifacts.size(); artifacts.clear(); return bytes;
        };
        for (String name : new String[] {"unknown", "", "../bad"}) {
            check(attached[0].deleteDexoptArtifacts(7, name, 1_010_100, 10) == -1);
            rejected(IllegalStateException.class, () -> last[0].getPackageStates());
        }
        for (int[] scope : new int[][] {{10100, 10}, {1_010_100, 0}})
            check(attached[0].deleteDexoptArtifacts(7, "p", scope[0], scope[1]) == -1);
        check(effects[0] == 0 && artifacts.size() == 18);
        long expected = artifacts.values().stream().mapToLong(Long::longValue).sum();
        check(attached[0].deleteDexoptArtifacts(7, "alias", 1_010_100, 10) == expected);
        check(effects[0] == 18 && artifacts.isEmpty());
        rejected(IllegalStateException.class, () -> last[0].getPackageState("p"));
        check(attached[0].deleteDexoptArtifacts(7, "p", 1_010_100, 10) == 0);
        for (var category : categories.entrySet()) {
            artifacts.put(category.getKey(), category.getValue());
            check(attached[0].deleteDexoptArtifacts(7, "p", 1_010_100, 10) == category.getValue());
            check(attached[0].deleteDexoptArtifacts(7, "p", 1_010_100, 10) == 0);
        }
        check(effects[0] == 36);
        // Original ArtManagerLocal logs artd RemoteException and returns zero, even after effects.
        ArtManagerLocal.deletion = (snapshot, name) -> 0L;
        check(attached[0].deleteDexoptArtifacts(7, "p", 1_010_100, 10) == 0);
        ArtManagerLocal.deletion = (snapshot, name) -> { last[0] = snapshot; throw new SecurityException("original error"); };
        rejected(SecurityException.class, () -> attached[0].deleteDexoptArtifacts(7, "p", 1_010_100, 10));
        rejected(IllegalStateException.class, () -> last[0].getPackageStates());
        int calls = ArtManagerLocal.calls;
        int kills = android.os.Process.kills;
        alive[0] = false;
        rejected(IllegalStateException.class, () -> attached[0].clearAppProfiles(7, "p"));
        rejected(IllegalStateException.class, () -> attached[0].deleteDexoptArtifacts(7, "p", 10100, 0));
        check(ArtManagerLocal.calls == calls && ArtManagerLocal.constructions == 1);
        check(android.os.Process.kills == kills + 2);
        rejected(IllegalStateException.class, local::withUnfilteredSnapshot);
        local.closeEpoch();
    }
}
