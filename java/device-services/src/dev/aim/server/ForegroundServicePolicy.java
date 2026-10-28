package dev.aim.server;

import android.app.ForegroundServiceTypePolicy;
import android.app.ForegroundServiceTypePolicy.ForegroundServiceTypePermission;
import android.content.Context;
import android.content.pm.ServiceInfo;
import android.os.Binder;
import android.os.RemoteException;
import android.provider.DeviceConfig;
import android.util.Slog;
import com.android.server.SystemConfig;

/** Retained framework policy; ActiveServices' small Java adapter (android-16.0.0_r1). */
final class ForegroundServicePolicy extends IForegroundServicePolicy.Stub {
    private final Context context;

    ForegroundServicePolicy(Context context) {
        this.context = context;
    }

    @Override
    public synchronized void validateForegroundServiceType(int type, int defaultToType, int startType,
            String packageName, int uid, int pid, boolean allowWhileInUse,
            String app, int targetSdk, IForegroundServiceTypeHost host) {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try {
            ForegroundServiceTypePolicy policy = ForegroundServiceTypePolicy.getDefaultPolicy();
            var info = policy.getForegroundServiceTypePolicyInfo(type, defaultToType);
            int effectiveType = info.getForegroundServiceType();
            if (effectiveType == ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED
                    || effectiveType == ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION) {
                info.setCustomPermission(new ForegroundServiceTypePermission(
                        effectiveType == ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED
                        ? "System exempted" : "Media projection screen capture permission") {
                    @Override
                    public int checkPermission(Context ignored, int callerUid, int callerPid,
                            String callerPackage, boolean wiu) {
                        boolean granted = effectiveType
                                == ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
                                ? fact(host, IForegroundServiceTypeHost.MEDIA_PROJECTION,
                                        callerUid, callerPackage)
                                : ForegroundServiceExemptions.systemExempted(callerUid,
                                        new ForegroundServiceExemptions.Source() {
                            public boolean has(int kind) {
                                return fact(host, kind, callerUid, callerPackage);
                            }
                            public boolean exemptedPackage() {
                                return SystemConfig.getInstance().getBgRestrictionExemption()
                                        .contains(callerPackage)
                                        || java.util.Arrays.asList(DeviceConfig.getString(
                                                "activity_manager", "bg_restriction_exempted_packages",
                                                "").split(",")).contains(callerPackage);
                            }
                            public boolean exemptPowerRestrictions() {
                                return DeviceConfig.getBoolean("activity_manager",
                                        "system_exempt_power_restrictions_enabled", true)
                                        && context.getSystemService(android.app.AppOpsManager.class)
                                        .checkOpNoThrow(128, callerUid, callerPackage) == 0;
                            }
                        });
                        return granted ? 0 : -1;
                    }
                });
            }
            try {
                int code = policy.checkForegroundServiceTypePolicy(context, packageName, uid,
                        pid, allowWhileInUse, info);
                String prefix = "Starting FGS with type "
                        + ServiceInfo.foregroundServiceTypeToLabel(type)
                        + " callerApp=" + app + " targetSDK=" + targetSdk;
                if (code == ForegroundServiceTypePolicy.FGS_TYPE_POLICY_CHECK_DISABLED) {
                        if (startType == ServiceInfo.FOREGROUND_SERVICE_TYPE_MANIFEST
                                && type == ServiceInfo.FOREGROUND_SERVICE_TYPE_NONE) {
                            throw new android.app.MissingForegroundServiceTypeException(
                                    "Starting FGS without a type  callerApp=" + app
                                    + " targetSDK=" + targetSdk);
                        }
                        throw new android.app.InvalidForegroundServiceTypeException(
                                prefix + " has been prohibited");
                } else if (code == ForegroundServiceTypePolicy.FGS_TYPE_POLICY_CHECK_PERMISSION_DENIED_ENFORCED) {
                        throw new SecurityException(prefix + " requires permissions: "
                                + info.toPermissionString() + (info.hasForegroundOnlyPermission()
                                ? " and the app must be in the eligible state/exemptions to access the foreground only permission"
                                : ""));
                } else if (code == ForegroundServiceTypePolicy.FGS_TYPE_POLICY_CHECK_DEPRECATED
                        || code == ForegroundServiceTypePolicy.FGS_TYPE_POLICY_CHECK_PERMISSION_DENIED_PERMISSIVE) {
                        Slog.w("AimForegroundService", prefix + " code=" + code
                                + " requiredPermissions=" + info.toPermissionString());
                }
            } finally {
                // The default policy is process-wide; never retain the host's Binder.
                info.setCustomPermission(null);
            }
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    /** ActiveServices.setServiceForegroundInnerLocked, before type validation. */
    @Override
    public boolean checkForegroundServiceStart(String packageName, int uid, int pid,
            boolean instant, int targetSdk) {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try {
            if (instant) {
                int mode = context.getSystemService(android.app.AppOpsManager.class).checkOpNoThrow(
                        android.app.AppOpsManager.OP_INSTANT_APP_START_FOREGROUND, uid, packageName);
                if (mode == 0) return true;
                if (mode == 1) return false;
                if (mode == 2) throw new SecurityException("Instant app " + packageName
                        + " does not have permission to create foreground services");
                context.enforcePermission("android.permission.INSTANT_APP_FOREGROUND_SERVICE",
                        pid, uid, "startForeground");
            } else if (targetSdk >= 28) {
                context.enforcePermission("android.permission.FOREGROUND_SERVICE", pid, uid,
                        "startForeground");
            }
            return true;
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    /** ServiceRecord.postNotification's missing-icon replacement, using original builders. */
    @Override
    public android.app.Notification repairForegroundServiceNotification(String packageName, int uid,
            android.app.Notification notification) {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try {
            if (notification.getSmallIcon() != null) return notification;
            int user = android.os.UserHandle.getUserId(uid);
            var pm = context.getPackageManager();
            var app = pm.getApplicationInfoAsUser(packageName, 0, user);
            CharSequence appName = app.loadLabel(pm);
            if (appName == null) appName = packageName;
            Context appContext = context.createPackageContextAsUser(packageName, 0,
                    android.os.UserHandle.of(user));
            var builder = new android.app.Notification.Builder(appContext, notification.getChannelId());
            builder.setSmallIcon(app.icon);
            builder.setFlag(64, true);
            var intent = new android.content.Intent("android.settings.APPLICATION_DETAILS_SETTINGS");
            intent.setData(android.net.Uri.fromParts("package", packageName, null));
            var pending = android.app.PendingIntent.getActivityAsUser(context, 0, intent,
                    (1 << 27) | (1 << 26), null, android.os.UserHandle.of(user));
            builder.setColor(context.getColor(com.android.internal.R.color.system_notification_accent_color));
            builder.setContentTitle(context.getString(
                    com.android.internal.R.string.app_running_notification_title, appName));
            builder.setContentText(context.getString(
                    com.android.internal.R.string.app_running_notification_text, appName));
            builder.setContentIntent(pending);
            return builder.build();
        } catch (android.content.pm.PackageManager.NameNotFoundException e) {
            return notification; // ServiceRecord leaves the original for its invalid-icon check.
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    private static boolean fact(IForegroundServiceTypeHost host, int kind, int uid, String pkg) {
        try {
            return host.hasForegroundServiceExemption(kind, uid, pkg);
        } catch (RemoteException e) {
            throw e.rethrowFromSystemServer();
        }
    }
}
