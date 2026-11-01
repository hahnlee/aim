package dev.aim.server;

import android.content.Intent;
import com.android.server.LocalServices;
import com.android.server.wm.ActivityTaskManagerInternal;

/** Local task queries and lifecycle updates backed by the native app host. */
final class TaskManagerLocal extends ActivityTaskManagerInternal {
    private final IAccessibilityWindowHost host;
    TaskManagerLocal(IAccessibilityWindowHost host) { this.host = host; }
    static void publish(ISystemProcess system) throws android.os.RemoteException {
        LocalServices.addService(ActivityTaskManagerInternal.class, new TaskManagerLocal(system.getAccessibilityWindowHost()));
    }

    @Override public void setAccessibilityServiceUids(android.util.IntArray uids) {
        try { host.setAccessibilityServiceUids(uids.toArray()); }
        catch (android.os.RemoteException e) { throw e.rethrowFromSystemServer(); }
    }

    @Override public void setAllowAppSwitches(String type, int uid, int userId) {
        try { host.setAllowAppSwitches(type, uid, userId); }
        catch (android.os.RemoteException e) { throw e.rethrowFromSystemServer(); }
    }

    @Override public void onUserStopped(int userId) {
        try { host.onUserStopped(userId); }
        catch (android.os.RemoteException e) { throw e.rethrowFromSystemServer(); }
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
