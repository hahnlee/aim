package dev.darwinart.runtime.pm;

import java.util.ArrayList;
import java.util.HashSet;

/** Immutable installation transport data, not parsed manifest or permission policy. */
public final class InstalledPackageRecord {
    public final String packageName;
    public final String baseApk;
    public final int appId;
    private final String[] splits;
    private final String metadata;
    private final String nativeLibraryDirectory;

    private InstalledPackageRecord(String name, String apk, int uid,
            ArrayList<String> splitPaths, String hints, String nativeDirectory) {
        packageName = name;
        baseApk = apk;
        appId = uid;
        splits = splitPaths.toArray(new String[0]);
        metadata = hints;
        nativeLibraryDirectory = nativeDirectory;
    }

    public static InstalledPackageRecord fromRecord(String name, String record) {
        if (record == null) return null;
        if (name == null || name.isEmpty() || name.length() > 255
                || name.startsWith(".") || name.endsWith(".") || name.contains("..")
                || !name.matches("[A-Za-z0-9_.-]+")) {
            throw new IllegalArgumentException("Invalid installed package name");
        }
        if (!record.startsWith("darwin-art-launch-v1\n") || !record.endsWith("\n")
                || record.indexOf('\0') >= 0 || record.length() > 60 * 1024) {
            throw new IllegalArgumentException("Invalid installed package record");
        }
        String apk = null;
        String uid = null;
        String hints = null;
        String nativeDirectory = null;
        ArrayList<String> splitPaths = new ArrayList<>();
        HashSet<String> seen = new HashSet<>();
        for (String line : record.split("\n")) {
            int delimiter = line.indexOf('=');
            if (delimiter < 0) continue;
            String key = line.substring(0, delimiter);
            String value = line.substring(delimiter + 1);
            if (key.equals("split")) {
                requireAbsolutePath(value);
                if (!splitPaths.contains(value)) splitPaths.add(value);
                else throw new IllegalArgumentException("Duplicate installed split");
            } else if (key.equals("apk") || key.equals("app_id") || key.equals("metadata")
                    || key.equals("native_library_dir")) {
                if (!seen.add(key)) throw new IllegalArgumentException("Duplicate installed field: " + key);
                if (key.equals("apk")) apk = value;
                if (key.equals("app_id")) uid = value;
                if (key.equals("metadata")) hints = value;
                if (key.equals("native_library_dir")) nativeDirectory = value;
            }
        }
        requireAbsolutePath(apk);
        if (nativeDirectory != null) requireAbsolutePath(nativeDirectory);
        if (splitPaths.contains(apk)) throw new IllegalArgumentException("Base APK also listed as split");
        int appId = uid == null ? -1 : Integer.parseInt(uid);
        // This transport is the profile's user-installed registry, whose ledger
        // allocates 10000..19999. System process identity is a different owner.
        if (uid != null && (appId < 10000 || appId > 19999)) {
            throw new IllegalArgumentException("App ID is outside the installed registry range");
        }
        InstalledPackageRecord result =
                new InstalledPackageRecord(name, apk, appId, splitPaths, hints, nativeDirectory);
        String declared = result.legacyManifestHint("package");
        if (declared != null && !name.equals(declared)) {
            throw new IllegalArgumentException("Package record identity mismatch");
        }
        return result;
    }

    private static void requireAbsolutePath(String path) {
        if (path == null || !path.startsWith("/") || path.indexOf('\r') >= 0) {
            throw new IllegalArgumentException("Missing or relative installed APK path");
        }
    }

    public String[] splitPaths() { return splits.clone(); }

    public boolean ownsPrimaryCodePath(String path) {
        if (baseApk.equals(path)) return true;
        for (String split : splits) if (split.equals(path)) return true;
        return false;
    }

    /** Current registry supports user zero; this does not synthesize manifest state. */
    public String dataDirectory() { return "/data/user/0/" + packageName; }

    /**
     * PackageManager always supplies LoadedApk a concrete native search
     * directory, including packages without JNI libraries. Older profile
     * records derive the install-owned directory from the sealed base APK.
     */
    public String nativeLibraryDirectory() {
        if (nativeLibraryDirectory != null) return nativeLibraryDirectory;
        int separator = baseApk.lastIndexOf('/');
        return baseApk.substring(0, separator) + "/android-elf/arm64-v8a";
    }

    /** True only when the installed Binary AndroidManifest declared the permission. */
    public boolean declaresPermission(String permission) {
        if (permission == null || permission.isEmpty()) return false;
        String declared = legacyManifestHint("permissions");
        if (declared == null || declared.equals("none")) return false;
        for (String name : declared.split(",")) {
            if (permission.equals(name)) return true;
        }
        return false;
    }

    /** Migration-only launcher hints. Not a substitute for AOSP manifest parsing. */
    String legacyManifestHint(String key) {
        String result = null;
        if (metadata == null) return null;
        for (String token : metadata.split(" ")) {
            if (!token.startsWith(key + "=")) continue;
            if (result != null) throw new IllegalArgumentException("Duplicate manifest hint: " + key);
            result = token.substring(key.length() + 1);
        }
        return result;
    }
}
