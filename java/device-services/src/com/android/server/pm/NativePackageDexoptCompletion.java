package com.android.server.pm;

import android.content.Context;
import android.os.RemoteException;
import android.util.ArraySet;
import com.android.internal.logging.MetricsLogger;
import com.android.internal.util.FrameworkStatsLog;
import com.android.server.LocalServices;
import com.android.server.pinner.PinnerService;
import com.android.server.art.ArtManagerLocal;
import com.android.server.art.ReasonMapping;
import com.android.server.art.model.DexoptResult;
import dev.aim.server.IPackageDexoptCompletion;
import java.io.File;
import java.util.Objects;
import java.util.concurrent.TimeUnit;

/** Original DexOptHelper completion, with native settings/usage instead of PMS. */
public final class NativePackageDexoptCompletion implements ArtManagerLocal.DexoptDoneCallback {
    private final Context context;
    private final CompilerStats compiler;
    private final Object packageLock;
    private final IPackageDexoptCompletion nativeOwner;
    public NativePackageDexoptCompletion(Context context, CompilerStats compiler,
            Object packageLock, IPackageDexoptCompletion nativeOwner) {
        this.context = Objects.requireNonNull(context);
        this.compiler = Objects.requireNonNull(compiler);
        this.packageLock = Objects.requireNonNull(packageLock);
        this.nativeOwner = Objects.requireNonNull(nativeOwner);
    }
    @Override public void onDexoptDone(DexoptResult result) {
        try {
            String reason = result.getReason();
            if (reason.equals(ReasonMapping.REASON_FIRST_BOOT)
                    || reason.equals(ReasonMapping.REASON_BOOT_AFTER_OTA)
                    || reason.equals(ReasonMapping.REASON_BOOT_AFTER_MAINLINE_UPDATE)) {
                int performed = 0, skipped = 0, failed = 0;
                for (var pkg : result.getPackageDexoptResults()) {
                    switch (pkg.getStatus()) {
                        case DexoptResult.DEXOPT_PERFORMED: performed++; break;
                        case DexoptResult.DEXOPT_SKIPPED: skipped++; break;
                        case DexoptResult.DEXOPT_FAILED: failed++; break;
                    }
                }
                long started = nativeOwner.getBootDexoptStartTimeNanos();
                MetricsLogger.histogram(context, "opt_dialog_num_dexopted", performed);
                MetricsLogger.histogram(context, "opt_dialog_num_skipped", skipped);
                MetricsLogger.histogram(context, "opt_dialog_num_failed", failed);
                MetricsLogger.histogram(context, "opt_dialog_num_total", nativeOwner.getOptimizablePackageCount());
                MetricsLogger.histogram(context, "opt_dialog_time_s",
                        (int) TimeUnit.NANOSECONDS.toSeconds(System.nanoTime() - started));
            }
            for (var pkg : result.getPackageDexoptResults()) {
                var stats = compiler.getOrCreatePackageStats(pkg.getPackageName());
                for (var dex : pkg.getDexContainerFileDexoptResults())
                    stats.setCompileTime(dex.getDexContainerFile(), dex.getDex2oatWallTimeMillis());
            }
            // Native disk publication owns its locks; do not hold the Java lock across Binder.
            nativeOwner.persistPackageUsage();
            synchronized (packageLock) { compiler.maybeWriteAsync(); }
            ArraySet<String> updated = new ArraySet<>();
            for (var pkg : result.getPackageDexoptResults()) {
                if (reason.equals(ReasonMapping.REASON_INACTIVE)
                        && pkg.getStatus() == DexoptResult.DEXOPT_PERFORMED) {
                    long before = 0, after = 0;
                    for (var dex : pkg.getDexContainerFileDexoptResults()) {
                        long source = new File(dex.getDexContainerFile()).length();
                        before += source + dex.getSizeBeforeBytes();
                        after += source + dex.getSizeBytes();
                    }
                    FrameworkStatsLog.write(FrameworkStatsLog.APP_DOWNGRADED,
                            pkg.getPackageName(), before, after, false);
                }
                if (pkg.hasUpdatedArtifacts()) updated.add(pkg.getPackageName());
            }
            if (!updated.isEmpty()) Objects.requireNonNull(LocalServices.getService(PinnerService.class)).update(updated, false);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
}
