package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageItemInfo;
import java.lang.reflect.Field;

/** Package-record to framework DTO boundary; never reads the launching app's environment. */
public final class InstalledApplicationInfo {
    private InstalledApplicationInfo() {}

    /**
     * Copies the PackageItemInfo label pair emitted by the APK inspector.
     *
     * PackageItemInfo gives the resource reference precedence only when
     * nonLocalizedLabel is null.  The inspector intentionally keeps the
     * package name in {@code label} when an application has no literal label,
     * so never install that fallback as a literal alongside a resource ID.
     */
    static void applyLabel(PackageItemInfo info, InstalledPackageRecord installed,
            String labelKey, String labelResKey) {
        String configuredResource = installed.legacyManifestHint(labelResKey);
        int labelRes = configuredResource == null || configuredResource.isEmpty()
                ? 0 : Integer.decode(configuredResource);
        info.labelRes = labelRes;

        String label = installed.legacyManifestHint(labelKey);
        if (labelRes == 0 && label != null && !label.isEmpty() && !label.equals("none")) {
            info.nonLocalizedLabel = label;
        } else {
            // Keep the two PackageItemInfo representations mutually exclusive.
            info.nonLocalizedLabel = null;
        }
    }

    private static void setCredentialProtectedDataDir(ApplicationInfo info, String path) {
        try {
            Field field = ApplicationInfo.class.getDeclaredField("credentialProtectedDataDir");
            field.setAccessible(true);
            field.set(info, path);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("ApplicationInfo credential storage is unavailable", error);
        }
    }

    public static ApplicationInfo fromRecord(String packageName, String record) {
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null) return null;
        ApplicationInfo info = new ApplicationInfo();
        info.packageName = packageName;
        info.processName = packageName;
        info.sourceDir = installed.baseApk;
        info.publicSourceDir = info.sourceDir;
        String[] splits = installed.splitPaths();
        if (splits.length != 0) {
            String encodedNames = installed.legacyManifestHint("split_names");
            if (encodedNames == null || encodedNames.isEmpty()) {
                throw new IllegalArgumentException("Installed split names are missing");
            }
            String[] names = encodedNames.split(",", -1);
            if (names.length != splits.length) {
                throw new IllegalArgumentException("Installed split names do not match split paths");
            }
            java.util.HashSet<String> seenNames = new java.util.HashSet<>();
            for (String name : names) {
                if (name.isEmpty() || !seenNames.add(name)) {
                    throw new IllegalArgumentException("Invalid or duplicate installed split name");
                }
            }
            info.splitNames = names;
            info.splitSourceDirs = splits;
            info.splitPublicSourceDirs = splits.clone();
        }
        info.dataDir = installed.dataDirectory();
        setCredentialProtectedDataDir(info, info.dataDir);
        info.deviceProtectedDataDir = "/data/user_de/0/" + packageName;
        info.nativeLibraryDir = installed.nativeLibraryDirectory();
        applyLabel(info, installed, "label", "label_res");
        // android:icon of <application>; PackageItemInfo.icon is the resource
        // apps use for notification small icons and launcher entries.
        String iconResource = installed.legacyManifestHint("icon_res");
        info.icon = iconResource == null || iconResource.isEmpty()
                ? 0 : Integer.decode(iconResource);
        String application = installed.legacyManifestHint("application");
        if (application != null && !application.equals("none")) info.className = application;
        info.metaData = InstalledManifestMetadata.fromRecord(installed);
        info.enabled = !"false".equals(installed.legacyManifestHint("enabled"));
        if ("true".equals(installed.legacyManifestHint("system"))) info.flags |= ApplicationInfo.FLAG_SYSTEM;
        if (!"0".equals(installed.legacyManifestHint("has_code"))) info.flags |= ApplicationInfo.FLAG_HAS_CODE;
        if ("1".equals(installed.legacyManifestHint("debuggable"))) info.flags |= ApplicationInfo.FLAG_DEBUGGABLE;
        if ("1".equals(installed.legacyManifestHint("hardware_accelerated"))) {
            info.flags |= ApplicationInfo.FLAG_HARDWARE_ACCELERATED;
        }
        // ApplicationInfo carries the manifest's supportsRtl decision as a
        // flag. Missing metadata preserves Android's default of false.
        if ("1".equals(installed.legacyManifestHint("supports_rtl"))) {
            info.flags |= ApplicationInfo.FLAG_SUPPORTS_RTL;
        }
        String target = installed.legacyManifestHint("target_sdk");
        info.targetSdkVersion = target == null ? 1 : Integer.parseInt(target);
        // Missing registration must not silently identify an app as root.
        info.uid = installed.appId;
        return info;
    }
}
