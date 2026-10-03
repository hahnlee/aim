import com.android.server.pm.parsing.PackageCacher;

public final class PackageRoundTripOracle {
    public static void main(String[] args) throws Exception {
        try {
            verify(args);
        } catch (Throwable failure) {
            failure.printStackTrace(System.out);
            System.exit(1);
        }
    }

    private static void verify(String[] args) throws Exception {
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
                byte[][] certificates = null;
                int[] capabilities = null;
                try (var in = new java.io.DataInputStream(new java.io.FileInputStream(file.getPath() + ".signing"))) {
                    int count = in.readInt();
                    if (count >= 0) {
                        certificates = new byte[count][];
                        capabilities = new int[count];
                        for (int i = 0; i < count; i++) {
                            certificates[i] = in.readNBytes(in.readInt());
                            capabilities[i] = in.readInt();
                        }
                    }
                    if (in.read() != -1) throw new AssertionError("trailing signing metadata");
                }
                var restored = dev.aim.server.PackageObjects.fromCache(
                    java.nio.file.Files.readAllBytes(file.toPath()), certificates, capabilities);
                var actual = restored.getSigningDetails();
                if (uid == 10000) {
                    if (!java.util.Arrays.equals(capabilities, new int[] {21, 23})) {
                        throw new AssertionError("pinned verified GSF lineage changed: " + java.util.Arrays.toString(capabilities));
                    }
                    for (int i = 0; i < capabilities.length; i++) {
                        var historical = actual.getPastSigningCertificates()[i];
                        if (historical.getFlags() != capabilities[i]
                            || signing.getPastSigningCertificates()[i].getFlags() != 0
                            || historical == signing.getPastSigningCertificates()[i]) {
                            throw new AssertionError("lineage flags or isolation lost");
                        }
                        if (i < capabilities.length - 1) {
                            var old = new android.content.pm.SigningDetails(
                                new android.content.pm.Signature[] {historical}, 3);
                            for (int mask : new int[] {1, 2, 4, 8, 16, 32, 15, 31}) {
                                if (actual.checkCapability(old, mask)
                                        != ((capabilities[i] & mask) == mask)) {
                                    throw new AssertionError("original capability predicate differs");
                                }
                            }
                            var revoked = dev.aim.server.PackageObjects.restoreSigning(signing,
                                certificates, new int[capabilities.length]);
                            if (revoked.checkCapability(old, 1)) throw new AssertionError("revocation lost");
                        }
                    }
                    rejects(signing, certificates, null);
                    rejects(signing, certificates, new int[0]);
                    rejects(signing, null, capabilities);
                    byte[][] wrong = certificates.clone();
                    wrong[0] = new byte[] {0};
                    rejects(signing, wrong, capabilities);
                    wrong[0] = null;
                    rejects(signing, wrong, capabilities);
                    capabilities[0] = 0;
                    certificates[0][0] ^= 1;
                    actual.getSignatures()[0].setFlags(123);
                    actual.getPublicKeys().clear();
                    if (actual.getPastSigningCertificates()[0].getFlags() != 21
                        || signing.getSignatures()[0].getFlags() != 0
                        || signing.getPublicKeys().size() != 1) {
                        throw new AssertionError("mutable signing inputs leaked");
                    }
                } else if (actual.getPastSigningCertificates() != null) {
                    throw new AssertionError("absent lineage became present");
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

    private static void rejects(android.content.pm.SigningDetails signing,
            byte[][] certificates, int[] capabilities) {
        try {
            dev.aim.server.PackageObjects.restoreSigning(signing, certificates, capabilities);
        } catch (IllegalArgumentException expected) {
            return;
        }
        throw new AssertionError("inconsistent lineage accepted");
    }
}
