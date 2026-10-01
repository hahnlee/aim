package dev.aim.server;

import android.Manifest;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.os.Binder;
import android.os.Build;
import android.os.RemoteException;
import android.os.UserHandle;
import android.util.Slog;

import com.android.server.LocalServices;
import com.android.server.policy.PermissionPolicyInternal;
import com.android.server.wm.ActivityInterceptorCallback;

import java.util.Arrays;

/**
 * Sends an app's request for POST_NOTIFICATIONS to the Mac's prompt
 * (#470): a temporary exception until PermissionController is native
 * (ADR 0013). A request for that permission alone, and the system's
 * request for a pre-33 app (PermissionPolicyService's
 * ACTION_REQUEST_PERMISSIONS_FOR_OTHER), start the invisible activity of
 * java/notification-permission in place of PermissionController's
 * dialog, with the same result. It gets a {@link Request} for the
 * requesting app, which asks the native service host; every other
 * request goes to PermissionController.
 */
final class NotificationPermissionInterceptor implements ActivityInterceptorCallback {
    private static final String TAG = "AimNotificationPermission";

    private static final ComponentName ACTIVITY = new ComponentName(
            "dev.aim.notificationpermission", "dev.aim.notificationpermission.RequestActivity");
    /** The {@link Request} in the activity's intent. */
    private static final String EXTRA_REQUEST = "dev.aim.extra.NOTIFICATION_PERMISSION_REQUEST";

    private final Context context;
    private final IServiceHost host;
    private final PermissionPolicyInternal policy;

    private NotificationPermissionInterceptor(Context context, IServiceHost host) {
        this.context = context;
        this.host = host;
        this.policy = LocalServices.getService(PermissionPolicyInternal.class);
    }

    static void register(Context context, IServiceHost host) {
        ProductInterceptor.add(new NotificationPermissionInterceptor(context, host));
    }

    @Override
    public ActivityInterceptResult onInterceptActivityLaunch(ActivityInterceptorInfo info) {
        Intent intent = info.getIntent();
        if (!policy.isIntentToPermissionDialog(intent)) {
            return null;
        }
        String[] names = intent.getStringArrayExtra(PackageManager.EXTRA_REQUEST_PERMISSIONS_NAMES);
        if (names == null || names.length == 0) {
            return null;
        }
        for (String name : names) {
            if (!Manifest.permission.POST_NOTIFICATIONS.equals(name)) {
                return null;
            }
        }
        // PermissionPolicyService lets only the system start the request
        // for another app (checkStartActivity).
        boolean system = PackageManager.ACTION_REQUEST_PERMISSIONS_FOR_OTHER
                .equals(intent.getAction());
        String packageName = system
                ? intent.getStringExtra(Intent.EXTRA_PACKAGE_NAME)
                : info.getCallingPackage();
        if (packageName == null) {
            return null;
        }
        Intent request = new Intent(intent).setPackage(null).setComponent(ACTIVITY);
        request.putExtra(EXTRA_REQUEST,
                new Request(context, host, packageName, info.getUserId(), system));
        return new ActivityInterceptResult(request, info.getCheckedOptions());
    }

    /** One app's request, which only the activity started for it holds. */
    private static final class Request extends INotificationPermissionRequest.Stub {
        private final Context context;
        private final IServiceHost host;
        private final String packageName;
        private final int userId;
        /** The system's request for a pre-33 app. */
        private final boolean system;

        Request(Context context, IServiceHost host, String packageName, int userId,
                boolean system) {
            this.context = context;
            this.host = host;
            this.packageName = packageName;
            this.userId = userId;
            this.system = system;
        }

        /**
         * Asks the Mac, where PermissionController would show its dialog
         * (NotificationGrantBehavior, BasicGrantBehavior).
         */
        @Override
        public void ask(INotificationPermissionCallback callback) throws RemoteException {
            PackageManager pm = context.getPackageManager();
            PackageInfo app;
            int flags;
            long token = Binder.clearCallingIdentity();
            try {
                app = pm.getPackageInfoAsUser(packageName, PackageManager.GET_PERMISSIONS, userId);
                flags = pm.getPermissionFlags(Manifest.permission.POST_NOTIFICATIONS, packageName,
                        UserHandle.of(userId));
            } catch (PackageManager.NameNotFoundException e) {
                callback.onResult(false);
                return;
            } finally {
                Binder.restoreCallingIdentity(token);
            }
            if (app.requestedPermissions == null || !Arrays.asList(app.requestedPermissions)
                    .contains(Manifest.permission.POST_NOTIFICATIONS)) {
                callback.onResult(false);
            } else if (!system
                    && app.applicationInfo.targetSdkVersion < Build.VERSION_CODES.TIRAMISU) {
                // An app below 33 cannot request it itself.
                callback.onIgnored();
            } else if ((flags & (PackageManager.FLAG_PERMISSION_POLICY_FIXED
                    | PackageManager.FLAG_PERMISSION_SYSTEM_FIXED)) != 0) {
                callback.onResult(context.checkPermission(Manifest.permission.POST_NOTIFICATIONS,
                        -1, app.applicationInfo.uid) == PackageManager.PERMISSION_GRANTED);
            } else {
                Slog.i(TAG, "asking the Mac for " + packageName);
                host.requestNotificationPermission(packageName, userId, callback);
            }
        }
    }
}
