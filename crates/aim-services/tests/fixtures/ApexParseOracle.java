import com.android.internal.pm.parsing.PackageParser2;
import com.android.server.pm.parsing.PackageCacher;

public final class ApexParseOracle {
    public static void verify(java.io.File directory) throws Exception {
        var features = new java.util.HashSet<>(java.nio.file.Files.readAllLines(new java.io.File(directory, "apex-parser-features.input").toPath()));
        try (var parser = new PackageParser2(null, null, null, new PackageParser2.Callback() {
            public boolean hasFeature(String feature) { return features.contains(feature); }
            public java.util.Set<String> getHiddenApiWhitelistedApps() { return java.util.Set.of(); }
            public java.util.Set<String> getInstallConstraintsAllowlist() { return java.util.Set.of(); }
            public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) { return false; }
        })) {
            var pm = android.content.pm.IPackageManager.Stub.asInterface(android.os.ServiceManager.getService("package"));
            var all = com.android.server.pm.ApexBootFeed.capture();
            java.nio.file.Files.write(new java.io.File(directory, "apex-inventory-parse.original").toPath(), all);
            // Actual original module paths and owner order, with InitAppsHelper's parse flags.
            var infos = android.os.Parcel.obtain();
            try {
                infos.unmarshall(all, 0, all.length); infos.setDataPosition(0);
                int count = infos.readInt();
                for (int i = 0; i < count; i++) {
                    infos.readString(); String path = infos.readString(); infos.readString(); infos.readLong();
                    infos.readBoolean(); infos.readBoolean(); infos.readBoolean();
                    var parsed = (com.android.internal.pm.parsing.pkg.PackageImpl) parser.parsePackage(new java.io.File(path), 1 << 4, false);
                    java.nio.file.Files.write(new java.io.File(directory, "apex-parse-" + i + ".original").toPath(), PackageCacher.toCacheEntryStatic(parsed));
                    var info = pm.getPackageInfo(parsed.getPackageName(), 0x48000000L, 0);
                    if (info == null || info.signingInfo == null) throw new AssertionError("missing original APEX signer: " + path);
                    if (info.applicationInfo == null || info.applicationInfo.uid != -1) throw new AssertionError("APEX acquired an application UID: " + path);
                    java.nio.file.Files.write(new java.io.File(directory, "apex-uid-" + i + ".original").toPath(), java.nio.ByteBuffer.allocate(4).order(java.nio.ByteOrder.LITTLE_ENDIAN).putInt(info.applicationInfo.uid).array());
                    var signing = android.os.Parcel.obtain();
                    try {
                        info.signingInfo.writeToParcel(signing, 0);
                        java.nio.file.Files.write(new java.io.File(directory, "apex-signing-" + i + ".original").toPath(), signing.marshall());
                    } finally { signing.recycle(); }
                    var past = info.signingInfo.getSigningDetails().getPastSigningCertificates();
                    var capabilities = android.os.Parcel.obtain();
                    try {
                        capabilities.writeInt(past == null ? -1 : past.length);
                        if (past != null) for (var certificate : past) capabilities.writeInt(certificate.getFlags());
                        java.nio.file.Files.write(new java.io.File(directory, "apex-capabilities-" + i + ".original").toPath(), capabilities.marshall());
                    } finally { capabilities.recycle(); }
                }
            } finally { infos.recycle(); }
        }
    }
    private ApexParseOracle() {}
}
