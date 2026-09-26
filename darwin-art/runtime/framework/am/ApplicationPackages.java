package dev.darwinart.runtime.am;

import android.app.AppGlobals;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.pm.PackageManagerInternal;
import android.content.pm.ParceledListSlice;
import android.content.pm.ProviderInfo;
import android.content.pm.ResolveInfo;
import android.content.pm.ServiceInfo;
import android.os.Process;
import android.os.RemoteException;
import android.os.UserHandle;
import com.android.server.LocalServices;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import java.util.List;

/**
 * The activity and task managers' view of installed packages, answered by
 * PackageManagerService as ActivityManagerService's mPackageManagerInt and
 * AppGlobals.getPackageManager() answer it.
 */
public final class ApplicationPackages implements PackageQueries {
    /** ActivityManagerService.STOCK_PM_FLAGS. */
    public static final long STOCK_PM_FLAGS = PackageManager.GET_SHARED_LIBRARY_FILES;

    public static final ApplicationPackages INSTANCE = new ApplicationPackages();

    private ApplicationPackages() {}

    private static PackageManagerInternal packageManager() {
        PackageManagerInternal packages = LocalServices.getService(PackageManagerInternal.class);
        if (packages == null) throw new IllegalStateException("PackageManagerService is not running");
        return packages;
    }

    /** The ApplicationInfo bindApplication carries, generated under STOCK_PM_FLAGS. */
    public static ApplicationInfo applicationInfo(String packageName, int uid) {
        return packageManager().getApplicationInfo(
                packageName, STOCK_PM_FLAGS, Process.SYSTEM_UID, UserHandle.getUserId(uid));
    }

    /** The package's MAIN/LAUNCHER activity, as a launcher resolves it. */
    public static ActivityInfo launchActivity(String packageName, int uid) {
        Intent intent = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
                .setPackage(packageName);
        List<ResolveInfo> resolved = packageManager().queryIntentActivities(intent, null,
                STOCK_PM_FLAGS, Process.SYSTEM_UID, UserHandle.getUserId(uid));
        return resolved == null || resolved.isEmpty() ? null : resolved.get(0).activityInfo;
    }

    /** ActivityTaskSupervisor.resolveActivity for an explicit component. */
    public static ActivityInfo activity(ComponentName component, int userId) {
        return packageManager().getActivityInfo(
                component, STOCK_PM_FLAGS, Process.SYSTEM_UID, userId);
    }

    @Override
    public ServiceInfo service(ComponentName component, int userId) {
        ResolveInfo resolved = packageManager().resolveService(
                new Intent().setComponent(component), null, STOCK_PM_FLAGS, userId,
                Process.SYSTEM_UID);
        return resolved == null ? null : resolved.serviceInfo;
    }

    @Override
    public void addIsolatedUid(int isolatedUid, int ownerUid) {
        packageManager().addIsolatedUid(isolatedUid, ownerUid);
    }

    @Override
    public void removeIsolatedUid(int isolatedUid) {
        packageManager().removeIsolatedUid(isolatedUid);
    }

    /**
     * ContentProviderHelper.generateApplicationProvidersLocked: the providers
     * declared for {@code processName}, in PMS's order.
     */
    public static List<ProviderInfo> processProviders(String processName, int uid)
            throws RemoteException {
        ParceledListSlice<ProviderInfo> providers = AppGlobals.getPackageManager()
                .queryContentProviders(processName, uid,
                        STOCK_PM_FLAGS | PackageManager.GET_URI_PERMISSION_PATTERNS
                                | PackageManager.MATCH_DIRECT_BOOT_AUTO, null);
        return providers == null ? new java.util.ArrayList<>() : providers.getList();
    }

    /** PermissionManagerService's grant state for {@code uid}. */
    public static boolean hasPermission(int uid, String permission) {
        PermissionManagerServiceInternal permissions =
                LocalServices.getService(PermissionManagerServiceInternal.class);
        if (permissions == null) throw new IllegalStateException("PermissionManagerService is not running");
        return permissions.checkUidPermission(uid, permission, Context.DEVICE_ID_DEFAULT)
                == PackageManager.PERMISSION_GRANTED;
    }
}
