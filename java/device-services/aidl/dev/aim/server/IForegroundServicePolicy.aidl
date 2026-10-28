package dev.aim.server;
import dev.aim.server.IForegroundServiceTypeHost;
import android.app.Notification;

/** Original framework policy, called only by the native system uid. */
interface IForegroundServicePolicy {
    void validateForegroundServiceType(int type, int defaultToType, int startType,
            String packageName, int uid, int pid, boolean allowWhileInUse,
            String app, int targetSdk, IForegroundServiceTypeHost host);
    boolean checkForegroundServiceStart(String packageName, int uid, int pid,
            boolean instant, int targetSdk);
    Notification repairForegroundServiceNotification(String packageName, int uid,
            in Notification notification);
    long[] getForegroundServiceTimeouts(int types, int uid);
    long startTimeLimitedForegroundService(int uid, int type, int previousType,
            boolean top, long lastTop, long uptime, long realtime, long limit);
    void stopTimeLimitedForegroundService(int uid, int type, long uptime);
    long timeoutTimeLimitedForegroundService(int uid, int type, boolean top,
            long lastTop, long uptime, long limit);
    void removeForegroundServiceUid(int uid);
    String foregroundServiceTypeLabel(int type);
}



