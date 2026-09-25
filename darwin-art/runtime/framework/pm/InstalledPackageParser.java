package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import com.android.internal.pm.parsing.PackageParser2;
import com.android.internal.pm.parsing.pkg.ParsedPackage;
import com.android.internal.pm.pkg.parsing.ParsingPackageUtils;
import com.android.server.SystemConfig;
import com.android.server.pm.pkg.AndroidPackage;
import dev.darwinart.runtime.compat.SystemCompatPolicy;
import java.io.File;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;

/**
 * Parses installed packages with the original AOSP PackageParser2 (manifest,
 * components, intent filters, permissions, meta-data) and APK signature
 * verification, as PackageManagerService scanning does.
 *
 * <p>The callback mirrors PackageManagerService's own parser callback:
 * PlatformCompat decides compat changes and SystemConfig answers features and
 * allowlists. Parsed packages are cached per installed record, so a replaced
 * install is parsed again.</p>
 */
final class InstalledPackageParser {
    private static final class Entry {
        final String record;
        final InstalledPackageRecord installed;
        final AndroidPackage pkg;

        Entry(String record, InstalledPackageRecord installed, AndroidPackage pkg) {
            this.record = record;
            this.installed = installed;
            this.pkg = pkg;
        }
    }

    /** An installed package: its ledger state and the parsed AOSP package. */
    static final class Parsed {
        final InstalledPackageRecord installed;
        final AndroidPackage pkg;

        Parsed(InstalledPackageRecord installed, AndroidPackage pkg) {
            this.installed = installed;
            this.pkg = pkg;
        }
    }

    private static final Map<String, Entry> cache = new HashMap<>();

    private InstalledPackageParser() {}

    private static final PackageParser2.Callback CALLBACK = new PackageParser2.Callback() {
        @Override
        public boolean isChangeEnabled(long changeId, ApplicationInfo appInfo) {
            return SystemCompatPolicy.isChangeEnabled(changeId, appInfo);
        }

        @Override
        public boolean hasFeature(String feature) {
            return SystemConfig.getInstance().getAvailableFeatures().containsKey(feature);
        }

        @Override
        public Set<String> getHiddenApiWhitelistedApps() {
            return SystemConfig.getInstance().getHiddenApiWhitelistedApps();
        }

        @Override
        public Set<String> getInstallConstraintsAllowlist() {
            return SystemConfig.getInstance().getInstallConstraintsAllowlist();
        }
    };

    /** Returns the parsed package for an installed record, or null when not installed. */
    static Parsed parse(String packageName, String record) {
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null) return null;
        synchronized (cache) {
            Entry cached = cache.get(packageName);
            if (cached != null && cached.record.equals(record)) {
                return new Parsed(cached.installed, cached.pkg);
            }
        }
        AndroidPackage pkg = parseInstalled(installed);
        synchronized (cache) {
            cache.put(packageName, new Entry(record, installed, pkg));
        }
        return new Parsed(installed, pkg);
    }

    private static AndroidPackage parseInstalled(InstalledPackageRecord installed) {
        String guestBase = InstalledPackageRecord.guestCodePath(installed.baseApk);
        if (guestBase == null) {
            throw new IllegalStateException("Installed code of " + installed.packageName
                    + " is outside this process's /data/app package mount");
        }
        File base = new File(guestBase);
        String[] hostSplits = installed.splitPaths();
        String[] splits = new String[hostSplits.length];
        for (int i = 0; i < hostSplits.length; i++) {
            splits[i] = InstalledPackageRecord.guestCodePath(hostSplits[i]);
            if (splits[i] == null) {
                throw new IllegalStateException("Installed split of " + installed.packageName
                        + " is outside the package mount");
            }
        }
        // A split install is an AOSP cluster: the base and its splits share
        // one code directory, as under /data/app.
        File codePath = base;
        if (splits.length != 0) {
            codePath = base.getParentFile();
            for (String split : splits) {
                if (!codePath.equals(new File(split).getParentFile())) {
                    throw new IllegalStateException(
                            "Installed splits are outside the base code directory");
                }
            }
        }
        ParsedPackage parsed;
        try (PackageParser2 parser = new PackageParser2(null, null, null, CALLBACK)) {
            parsed = parser.parsePackage(codePath,
                    ParsingPackageUtils.PARSE_COLLECT_CERTIFICATES, false);
        } catch (RuntimeException error) {
            throw error;
        } catch (Exception error) {
            // PackageManagerService logs scan failures the same way.
            android.util.Log.w("PackageManager", "Failed to parse " + codePath, error);
            throw new IllegalStateException("Failed to parse installed package "
                    + installed.packageName + ": " + error, error);
        }
        if (!installed.packageName.equals(parsed.getPackageName())) {
            throw new IllegalStateException("Installed APK declares package "
                    + parsed.getPackageName() + ", not " + installed.packageName);
        }
        if (!base.getPath().equals(parsed.getBaseApkPath())) {
            throw new IllegalStateException("Parsed base APK does not match the installed record");
        }
        String[] parsedSplits = parsed.getSplitCodePaths();
        java.util.Set<String> expected = new java.util.HashSet<>(java.util.Arrays.asList(splits));
        java.util.Set<String> actual = new java.util.HashSet<>(parsedSplits == null
                ? java.util.Collections.<String>emptyList() : java.util.Arrays.asList(parsedSplits));
        if (!expected.equals(actual)) {
            throw new IllegalStateException("Parsed splits do not match the installed record");
        }
        return parsed.hideAsFinal();
    }
}
