import com.android.server.pm.parsing.PackageCacher;

/** Original constructor decisions for corrupted native cache fields. */
final class PackageCacheValidationOracle {
    static void verify(java.io.File root) throws Exception {
        for (String name : new String[] {"empty", "populated", "empty-process-name"}) {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(root, "cache-validation-" + name).toPath());
            var parsed = PackageCacher.fromCacheEntryStatic(bytes);
            if (!java.util.Arrays.equals(bytes, PackageCacher.toCacheEntryStatic(parsed))) {
                throw new AssertionError("valid cache owner changed: " + name);
            }
            if (name.equals("empty-process-name")) {
                var processes = ((com.android.internal.pm.parsing.pkg.PackageImpl)parsed).getProcesses();
                if (processes.size() != 1 || !processes.containsKey(null)
                        || !"".equals(processes.get(null).getName())) throw new AssertionError("nullable map key and empty value name differ");
            }
        }
        for (String name : new String[] {"null-array", "null-string", "null-process-name"}) {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(root, "cache-validation-" + name).toPath());
            try {
                PackageCacher.fromCacheEntryStatic(bytes);
                throw new AssertionError("original constructor accepted " + name);
            } catch (NullPointerException expected) {}
        }
    }
}
