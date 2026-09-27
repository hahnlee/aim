package dev.darwinart.runtime.system;

import android.util.Slog;
import com.android.server.SystemServiceManager;
import com.android.server.am.ActivityManagerService;
import com.android.server.pm.Installer;
import com.android.server.uri.UriGrantsManagerService;
import com.android.server.wm.ActivityTaskManagerService;

/**
 * Opt-in bring-up of the original ActivityTaskManagerService and
 * ActivityManagerService (DARWIN_ART_UPSTREAM_ACTIVITY_MANAGER=1, #148), in
 * SystemServer.startBootstrapServices order, in place of the runtime's own
 * activity endpoints. Construction failures are fatal (there is no fallback
 * owner); later steps log their outcome so a disposable profile reports the
 * blockers in order.
 */
public final class UpstreamActivityManager {
    private static final String TAG = "UpstreamActivityManager";
    private static final boolean ENABLED =
            "1".equals(System.getenv("DARWIN_ART_UPSTREAM_ACTIVITY_MANAGER"));

    private UpstreamActivityManager() {}

    public static boolean enabled() {
        return ENABLED;
    }

    /**
     * SystemServer: UriGrantsManagerService (after Installer), then ATMS and
     * AMS after AccessCheckingService.
     */
    public static ActivityManagerService start(SystemServiceManager services,
            Installer installer) {
        services.startService(UriGrantsManagerService.Lifecycle.class);
        Slog.i(TAG, "step ok: UriGrantsManagerService.Lifecycle");
        ActivityTaskManagerService atm = ((ActivityTaskManagerService.Lifecycle)
                services.startService(ActivityTaskManagerService.Lifecycle.class)).getService();
        Slog.i(TAG, "step ok: ActivityTaskManagerService.Lifecycle");
        ActivityManagerService activity = ActivityManagerService.Lifecycle.startService(services, atm);
        Slog.i(TAG, "step ok: ActivityManagerService.Lifecycle");
        activity.setSystemServiceManager(services);
        activity.setInstaller(installer);
        return activity;
    }

    /** A later SystemServer step on the upstream owner; false when it threw. */
    public static boolean step(String name, Runnable step) {
        try {
            step.run();
            Slog.i(TAG, "step ok: " + name);
            return true;
        } catch (Throwable error) {
            Slog.e(TAG, "step failed: " + name, error);
            return false;
        }
    }
}
