package dev.aim.server;

import android.content.Context;
import android.content.pm.IPackageLoadingProgressCallback;
import android.content.pm.PackageManagerInternal;
import android.os.Binder;
import android.os.incremental.IncrementalManager;
import android.util.Slog;
import java.util.Objects;

/** Original IncrementalManager leaf; package existence/filter/loading come from native capture. */
public final class NativePackageLoadingProgress {
    private final Context context;
    private final PackageSnapshots.Store packages;
    public NativePackageLoadingProgress(Context context, PackageSnapshots.Store packages) {
        this.context = Objects.requireNonNull(context);
        this.packages = Objects.requireNonNull(packages);
    }
    public boolean register(String name, PackageManagerInternal.InstalledLoadingProgressCallback callback,
            int user) {
        try (var snapshot = packages.computer()) {
            var state = snapshot.getPackageStateForInstalledAndFiltered(name, Binder.getCallingUid(), user);
            if (state == null) return false;
            if (!state.isLoading()) {
                Slog.w("PackageManager", "Failed registering loading progress callback. Package is fully loaded.");
                return false;
            }
            IncrementalManager incremental = context.getSystemService(IncrementalManager.class);
            if (incremental == null) {
                Slog.w("PackageManager", "Failed registering loading progress callback. Incremental is not enabled");
                return false;
            }
            return incremental.registerLoadingProgressCallback(state.getPathString(),
                    (IPackageLoadingProgressCallback) callback.getBinder());
        }
    }
}
