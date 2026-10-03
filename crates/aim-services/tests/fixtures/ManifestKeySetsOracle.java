import com.android.internal.pm.parsing.PackageParser2;
import com.android.internal.pm.parsing.PackageParserException;
import com.android.internal.pm.parsing.pkg.PackageImpl;

public final class ManifestKeySetsOracle {
    private static String hex(byte[] bytes) {
        StringBuilder text = new StringBuilder();
        for (byte b : bytes) text.append(String.format("%02x", b & 255));
        return text.toString();
    }

    public static void main(String[] args) throws Exception {
        try (var parser = new PackageParser2(null, null, null, new PackageParser2.Callback() {
            public boolean hasFeature(String feature) { return false; }
            public java.util.Set<String> getHiddenApiWhitelistedApps() { return java.util.Set.of(); }
            public java.util.Set<String> getInstallConstraintsAllowlist() { return java.util.Set.of(); }
            public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) { return false; }
        })) {
            for (String path : args) {
                java.io.File file = new java.io.File(path);
                PackageImpl pkg;
                try {
                    pkg = (PackageImpl) parser.parsePackage(file, 0, false);
                } catch (PackageParserException error) {
                    System.out.println("CASE " + file.getName() + " ERROR");
                    continue;
                }
                System.out.println("CASE " + file.getName() + " OK");
                for (var entry : pkg.getKeySetMapping().entrySet()) {
                    for (var key : entry.getValue()) {
                        var bytes = new java.io.ByteArrayOutputStream();
                        try (var out = new java.io.ObjectOutputStream(bytes)) { out.writeObject(key); }
                        System.out.println("SET " + entry.getKey() + " " + key.getClass().getName()
                            + " " + hex(key.getEncoded()) + " " + hex(bytes.toByteArray()));
                    }
                }
                for (String upgrade : pkg.getUpgradeKeySets()) System.out.println("UPGRADE " + upgrade);
            }
        }
    }
}
