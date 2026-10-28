package dev.aim.server;

import android.content.Intent;
import com.android.server.LocalServices;
import com.android.server.wm.ActivityTaskManagerInternal;

/** Cold local queries for the app host's normal (non-factory-test) boot. */
final class TaskManagerLocal extends ActivityTaskManagerInternal {
    static void publish() {
        LocalServices.addService(ActivityTaskManagerInternal.class, new TaskManagerLocal());
    }

    // ActivityTaskManagerService.getHomeIntent, android-16.0.0_r1 (Apache-2.0).
    // The app host has no factory-test top action, data or component override.
    @Override public Intent getHomeIntent() {
        return new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME)
                .addFlags(Intent.FLAG_DEBUG_TRIAGED_MISSING);
    }

    private static UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("ActivityTaskManagerInternal outside HOME queries (#2274)");
    }
    @Override public void registerActivityStartInterceptor(int id,
            com.android.server.wm.ActivityInterceptorCallback callback) { throw unsupported(); }
    @Override public PackageConfigurationUpdater createPackageConfigurationUpdater(
            String packageName, int userId) { throw unsupported(); }
    @Override public boolean isBaseOfLockedTask(String name) { throw unsupported(); }
}
