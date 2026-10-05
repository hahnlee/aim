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
    }
    private static final class Owner extends dev.aim.server.IPackageScanSnapshot.Stub {
        private final File directory;
        boolean omitOriginal;
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
        @Override public long getVersion() { return Long.parseLong(lines("version")[0]); }
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
        @Override public void close() { closes++; }
    }
}
