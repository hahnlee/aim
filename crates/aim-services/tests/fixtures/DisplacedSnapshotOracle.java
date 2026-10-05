import java.io.File;
import java.nio.file.Files;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

public final class DisplacedSnapshotOracle {
    private static final String INCOMING = "com.google.android.gsf";
    private static final String ORIGINAL = "fixture.original.gsf";
    public static void main(String[] args) {
        try {
            for (int i = 0; i < 6; i++) verify(new File(args[0], "case-" + i), i);
            System.out.println("displaced full snapshot contracts: 6 cases");
            System.exit(0);
        } catch (Throwable error) { error.printStackTrace(System.out); System.exit(1); }
    }
    private static void verify(File directory, int index) throws Exception {
        var endpoint = new Owner(directory);
        var visibility = new dev.aim.server.PackageSnapshots.Owner() {
            @Override public String getFilteredPackageName(long version, String candidate, int uid, int user) { throw new UnsupportedOperationException("unfiltered fixture"); }
            @Override public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) { throw new UnsupportedOperationException("unfiltered fixture"); }
        };
        dev.aim.server.PackageSnapshots.Data data;
        try (var lease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(endpoint))) {
            data = lease.captureData(visibility, true);
        }
        try (var snapshot = dev.aim.server.PackageSnapshots.unfiltered(data)) {
            var incoming = snapshot.getPackageStates().get(INCOMING);
            var original = snapshot.getPackageStates().get(ORIGINAL);
            if (incoming == null || original == null || incoming.getAndroidPackage() != null
                    || original.getAndroidPackage() == null || incoming.getSharedUserAppId() != 10002
                    || original.getAppId() != 10003
                    || !INCOMING.equals(((com.android.server.pm.pkg.PackageStateInternal) original).getRealName()))
                throw new AssertionError("displaced identities differ: " + index);
            // Cases cover independent/shared targets; each has pruned/active/disabled old groups.
            int keep = index % 3;
            boolean shared = index >= 3;
            var oldGroup = snapshot.getSharedUsers().get("fixture.incoming.group");
            if ((oldGroup != null) != (keep != 0)) throw new AssertionError("old group presence differs");
            if (oldGroup != null && (oldGroup.getAppId() != 10002
                    || oldGroup.getPackageStates().contains(incoming)
                    || oldGroup.getPackageStates().size() != (keep == 1 ? 1 : 0)))
                throw new AssertionError("displaced request became a group member");
            if (snapshot.getDisabledSystemPackageStates().size() != (keep == 2 ? 1 : 0))
                throw new AssertionError("factory inventory differs");
            if (shared) {
                var group = snapshot.getSharedUsers().get("fixture.original.group");
                if (group == null || group.getPackageStates().size() != 2
                        || !group.getPackageStates().contains(original)) throw new AssertionError("original retained group differs");
                int retained = 0;
                for (var member : group.getPackageStates()) {
                    if (!ORIGINAL.equals(member.getPackageName())) throw new AssertionError("original member name differs");
                    if (member != original && member.getAndroidPackage() == null) retained++;
                }
                if (retained != 1) throw new AssertionError("original instances collapsed");
            }
        }
        if (endpoint.closes != 1) throw new AssertionError("lease did not close its endpoint");
        endpoint.omitOriginal = true;
        try (var lease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(endpoint))) {
            try { lease.captureData(visibility, true); throw new AssertionError("missing accepted original allowed"); }
            catch (java.io.IOException expected) {}
        }
        verifyStore(directory, new File(directory.getParentFile(), directory.getName() + "-next"), visibility);
    }
    private static void verifyStore(File first, File next, dev.aim.server.PackageSnapshots.Owner visibility) throws Exception {
        Owner[] source = { new Owner(first) };
        var store = new dev.aim.server.PackageSnapshots.Store(
                () -> source[0] == null ? null : dev.aim.server.IPackageScanSnapshot.Stub.asInterface(source[0]), visibility, true);
        try { store.unfiltered(); throw new AssertionError("uninitialized store accepted"); }
        catch (IllegalStateException expected) {}
        long version = source[0].getVersion();
        if (store.refresh() != version || source[0].closes != 1) throw new AssertionError("initial refresh differs");
        try (var old = store.unfiltered()) {
            var original = old.getPackageStates().get(ORIGINAL);
            source[0] = new Owner(first);
            if (store.refresh() != version || source[0].closes != 1) throw new AssertionError("same-version lease leaked");
            try (var same = store.unfiltered()) {
                if (same.getPackageStates().get(ORIGINAL) != original) throw new AssertionError("same-version identity changed");
            }
            source[0] = new Owner(next);
            source[0].omitOriginal = true;
            try { store.refresh(); throw new AssertionError("partial newer graph published"); }
            catch (java.io.IOException expected) {}
            if (store.getVersion() != version || source[0].closes != 1) throw new AssertionError("failed capture changed store");
            source[0] = new Owner(next);
            source[0].failClose = true;
            try { store.refresh(); throw new AssertionError("failed close published graph"); }
            catch (IllegalStateException expected) {}
            if (store.getVersion() != version || source[0].closes != 1) throw new AssertionError("failed close changed store");
            source[0] = new Owner(next);
            if (store.refresh() != version + 1 || source[0].closes != 1) throw new AssertionError("newer retry differs");
            try (var newer = store.unfiltered()) {
                if (newer.getPackageStates().get(ORIGINAL) == original) throw new AssertionError("different versions reused a replica");
                if (old.getPackageStates().get(ORIGINAL) != original) throw new AssertionError("old scope changed");
            }
            source[0] = new Owner(first);
            try { store.refresh(); throw new AssertionError("backwards version accepted"); }
            catch (java.io.IOException expected) {}
            if (source[0].closes != 1) throw new AssertionError("stale lease leaked");
            source[0] = new Owner(next);
            source[0].failVersion = true;
            source[0].failClose = true;
            try { store.refresh(); throw new AssertionError("failed initial version accepted"); }
            catch (IllegalStateException expected) {
                if (expected.getSuppressed().length != 1) throw new AssertionError("initial close failure lost");
            }
            if (source[0].closes != 1) throw new AssertionError("initial failure lease leaked");
            source[0] = new Owner(next);
            source[0].versionOverride = 0L;
            try { store.refresh(); throw new AssertionError("invalid version accepted"); }
            catch (IllegalStateException expected) {}
            if (source[0].closes != 1) throw new AssertionError("invalid version lease leaked");
            source[0] = null;
            try { store.refresh(); throw new AssertionError("missing endpoint accepted"); }
            catch (java.io.IOException expected) {}
            if (store.getVersion() != version + 1 || old.getPackageStates().get(ORIGINAL) != original)
                throw new AssertionError("rejected refresh replaced state");
        }
    }
    private static final class Owner extends dev.aim.server.IPackageScanSnapshot.Stub {
        private final File directory;
        boolean omitOriginal;
        boolean failVersion;
        boolean failClose;
        Long versionOverride;
        int closes;
        Owner(File directory) { this.directory = directory; }
        private byte[] read(String path) {
            var file = new File(directory, path);
            if (!file.exists()) return null;
            try { return Files.readAllBytes(file.toPath()); }
            catch (java.io.IOException error) { throw new java.io.UncheckedIOException(error); }
        }
        private String scope(String name, boolean factory) { return (factory ? "factory/" : "active/") + name + "/"; }
        private String[] lines(String path) { return new String(java.util.Objects.requireNonNull(read(path)), StandardCharsets.UTF_8).lines().toArray(String[]::new); }
        private int length(String path) { byte[] bytes = read(path); return bytes == null ? -1 : bytes.length; }
        private byte[] chunk(String path, int offset, int length) { return Arrays.copyOfRange(java.util.Objects.requireNonNull(read(path)), offset, offset + length); }
        @Override public android.os.IInterface queryLocalInterface(String descriptor) { return null; }
        @Override public long getVersion() {
            if (failVersion) throw new IllegalStateException("fixture version failure");
            return versionOverride == null ? Long.parseLong(lines("version")[0]) : versionOverride;
        }
        @Override public String[] getPackageNames(boolean factory) {
            return Arrays.stream(lines(factory ? "factory-names" : "active-names")).filter(n -> factory || !omitOriginal || !n.equals(ORIGINAL)).toArray(String[]::new);
        }
        @Override public int getCodeLength(String n, boolean f) { return length(scope(n,f)+"code"); }
        @Override public byte[] getCodeChunk(String n, boolean f, int o, int l) { return chunk(scope(n,f)+"code",o,l); }
        @Override public int getRuntimeStateLength(String n, boolean f) { return length(scope(n,f)+"runtime"); }
        @Override public byte[] getRuntimeStateChunk(String n, boolean f, int o, int l) { return chunk(scope(n,f)+"runtime",o,l); }
        @Override public int getSettingLength(String n, boolean f) { return length(scope(n,f)+"setting"); }
        @Override public byte[] getSettingChunk(String n, boolean f, int o, int l) { return chunk(scope(n,f)+"setting",o,l); }
        @Override public byte[] getSigningState(String n, boolean f) { return read(scope(n,f)+"signing"); }
        @Override public byte[] getTransientState(String n, boolean f) { return read(scope(n,f)+"transient"); }
        @Override public int getHiddenApiEnforcementPolicy(String n, boolean f) { return Integer.parseInt(lines(scope(n,f)+"hidden")[0]); }
        @Override public int[] getUserStateIds(String n, boolean f) { return Arrays.stream(lines(scope(n,f)+"users")).mapToInt(Integer::parseInt).toArray(); }
        @Override public int getUserStateLength(String n, boolean f, int u) { return length(scope(n,f)+"user-"+u); }
        @Override public byte[] getUserStateChunk(String n, boolean f, int u, int o, int l) { return chunk(scope(n,f)+"user-"+u,o,l); }
        @Override public String[] getSharedUserNames() { return lines("groups"); }
        @Override public int getSharedUserStateLength(String n) { return length("group/"+n); }
        @Override public byte[] getSharedUserStateChunk(String n, int o, int l) { return chunk("group/"+n,o,l); }
        @Override public byte[] getUsage(String n) { throw new UnsupportedOperationException("scoped runtime owns usage"); }
        @Override public byte[] getSeInfo(String n) { throw new UnsupportedOperationException("scoped runtime owns seInfo"); }
        @Override public int getLibraryStateLength(String n) { throw new UnsupportedOperationException("scoped runtime owns libraries"); }
        @Override public byte[] getLibraryStateChunk(String n, int o, int l) { throw new UnsupportedOperationException("scoped runtime owns libraries"); }
        @Override public void close() { closes++; if (failClose) throw new IllegalStateException("fixture close failure"); }
    }
}
