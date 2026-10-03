import com.android.server.pm.parsing.PackageCacher;

public final class PackageRoundTripOracle {
    public static void main(String[] args) throws Exception {
        var files = new java.io.File(args[0]).listFiles((dir, name) -> name.endsWith(".native"));
        if (files == null) throw new java.io.IOException("missing parcel inputs");
        java.util.Arrays.sort(files);
        for (var file : files) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                PackageCacher.fromCacheEntryStatic(java.nio.file.Files.readAllBytes(file.toPath()));
            if (file.getName().startsWith("scan-")) {
                int uid = Integer.parseInt(file.getName().substring(5, file.getName().indexOf('.')));
                var signing = pkg.getSigningDetails();
                if (pkg.getUid() != uid || signing.getSignatureSchemeVersion() != 3
                    || signing.getSignatures().length != 1 || signing.getPublicKeys().size() != 1
                    || (uid == 10000 && signing.getPastSigningCertificates().length != 2)) {
                    throw new AssertionError("native scan UID/signing was not finalized: " + file.getName());
                }
            }
            if (file.getName().contains("-true.native") && (pkg.getUid() != 19001
                || !"arm64-v8a".equals(pkg.getPrimaryCpuAbi())
                || !"/data/app/fixture/lib".equals(pkg.getNativeLibraryRootDir())
                || pkg.getPageSizeAppCompatFlags() != 8)) {
                throw new AssertionError("native scan metadata was not decoded: " + file.getName());
            }
            java.nio.file.Files.write(new java.io.File(file.getPath() + ".original").toPath(),
                PackageCacher.toCacheEntryStatic(pkg));
        }
        System.out.println("PARCELS " + files.length);
    }
}
