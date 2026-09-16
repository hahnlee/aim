package dev.darwinart.runtime.pm;

import com.android.server.pm.dex.DexUsageStore;
import java.lang.reflect.InvocationTargetException;
import java.util.Map;

/** PM validation/ownership boundary; original PackageDexUsage owns merging and persistence. */
public final class DexLoadReports {
    @FunctionalInterface
    public interface CallerIdentity {
        /** True only when system-owned process state resolves an isolated UID to the package. */
        boolean isCallerSameApp(int callerUid, String packageName);
    }

    private final PackageRecords.Source packages;
    private final DexUsageStore usage;
    private final CallerIdentity callerIdentity;

    public DexLoadReports(PackageRecords.Source packages, DexUsageStore usage) {
        this(packages, usage, null);
    }

    public DexLoadReports(PackageRecords.Source packages, DexUsageStore usage,
            CallerIdentity identity) {
        this.packages = packages;
        this.usage = usage;
        callerIdentity = identity;
    }

    public void report(int callerUid, String packageName, Map<String, String> contexts, String isa) {
        if (packageName == null) throw new IllegalArgumentException("Missing loading package");
        String record = packages.resolveInstalledPackage(packageName);
        InstalledPackageRecord app = InstalledPackageRecord.fromRecord(packageName, record);
        boolean directOwner = callerUid == 0 || callerUid == 1000
                || (app != null && app.appId >= 0 && callerUid == app.appId);
        boolean isolatedOwner = callerIdentity != null
                && callerIdentity.isCallerSameApp(callerUid, packageName);
        if (app == null || app.appId < 0 || callerUid < 0 || (!directOwner && !isolatedOwner)) {
            throw new SecurityException("DEX report caller does not own the loading package");
        }
        if (contexts == null) return; // Original legacy DexManager contract.
        if (contexts.isEmpty() || !DexInstructionSets.checkISA(isa)) {
            throw new IllegalArgumentException("Invalid DEX report or instruction set");
        }
        for (Map.Entry<String, String> entry : contexts.entrySet()) {
            String path = entry.getKey();
            if (path == null) throw new IllegalArgumentException("Missing DEX path");
            boolean primary = app.ownsPrimaryCodePath(path);
            // Legacy DexManager skips a package loading its own primary/split.
            boolean platform = "android".equals(packageName);
            if (primary && !platform) continue;
            // Unknown ownership must not be attributed to the reporting app.
            // Registry-wide shared-DEX discovery remains a separate PM task.
            if (!primary && (!path.startsWith(app.dataDirectory() + "/") || path.contains("/../") || path.endsWith("/.."))) continue;
            String context = entry.getValue();
            if (context != null && validContext(context)) {
                usage.record(packageName, path, app.appId / 100000, isa, primary,
                        packageName, context, platform);
            }
        }
    }

    private static boolean validContext(String context) {
        try {
            return (Boolean) Class.forName("dalvik.system.VMRuntime")
                    .getMethod("isValidClassLoaderContext", String.class).invoke(null, context);
        } catch (InvocationTargetException error) {
            Throwable cause = error.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            if (cause instanceof Error) throw (Error) cause;
            throw new IllegalStateException(cause);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Missing ART class-loader-context validator", error);
        }
    }
}
