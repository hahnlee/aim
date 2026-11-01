// Compile-only android-16.0.0_r1 contract; checked against the image.
package android.app.usage;
import android.content.ComponentName;
import android.content.LocusId;
import android.content.res.Configuration;
import android.os.IBinder;
import android.os.PersistableBundle;
import android.os.UserHandle;
import java.util.List;
import java.util.Set;
public abstract class UsageStatsManagerInternal {
    public abstract void reportEvent(ComponentName component, int userId, int eventType, int instanceId, ComponentName taskRoot);
    public abstract void reportEvent(String packageName, int userId, int eventType);
    public abstract void reportEventForAllUsers(String packageName, int eventType);
    public abstract void reportConfigurationChange(Configuration config, int userId);
    public abstract void reportInterruptiveNotification(String packageName, String channelId, int userId);
    public abstract void reportShortcutUsage(String packageName, String shortcutId, int userId);
    public abstract void reportContentProviderUsage(String name, String pkgName, int userId);
    public abstract void reportLocusUpdate(ComponentName activity, int userId, LocusId locusId, IBinder appToken);
    public abstract void reportUserInteractionEvent(String pkgName, int userId, PersistableBundle extras);
    public abstract void prepareShutdown();
    public abstract void prepareForPossibleShutdown();
    public abstract boolean isAppIdle(String packageName, int uidForAppId, int userId);
    public abstract int getAppStandbyBucket(String packageName, int userId, long nowElapsed);
    public abstract int[] getIdleUidsForUser(int userId);
    public abstract byte[] getBackupPayload(int userId, String key);
    public abstract void applyRestoredPayload(int userId, String key, byte[] payload);
    public abstract void onActiveAdminAdded(String packageName, int userId);
    public abstract void setActiveAdminApps(Set<String> adminApps, int userId);
    public abstract void setAdminProtectedPackages(Set<String> packageNames, int userId);
    public abstract void onAdminDataAvailable();
    public abstract List<UsageStats> queryUsageStatsForUser(int userId, int interval, long beginTime, long endTime, boolean obfuscateInstantApps);
    public abstract UsageEvents queryEventsForUser(int userId, long beginTime, long endTime, int flags);
    public abstract void setLastJobRunTime(String packageName, int userId, long elapsedRealtime);
    public abstract long getEstimatedPackageLaunchTime(String packageName, int userId);
    public abstract long getTimeSinceLastJobRun(String packageName, int userId);
    public abstract void reportSyncScheduled(String packageName, int userId, boolean exempted);
    public abstract void reportExemptedSyncStart(String packageName, int userId);
    public abstract AppUsageLimitData getAppUsageLimit(String packageName, UserHandle user);
    public abstract boolean pruneUninstalledPackagesData(int userId);
    public abstract boolean updatePackageMappingsData(int userId);
    public abstract void registerListener(UsageEventListener listener);
    public abstract void unregisterListener(UsageEventListener listener);
    public abstract void registerLaunchTimeChangedListener( EstimatedLaunchTimeChangedListener listener);
    public abstract void reportBroadcastDispatched(int sourceUid, String targetPackage, UserHandle targetUser, long idForResponseEvent, long timestampMs, int targetUidProcState);
    public abstract void reportNotificationPosted(String packageName, UserHandle user, long timestampMs);
    public abstract void reportNotificationUpdated(String packageName, UserHandle user, long timestampMs);
    public abstract void reportNotificationRemoved(String packageName, UserHandle user, long timestampMs);
    public static class AppUsageLimitData { public AppUsageLimitData(long total, long remaining) { throw new RuntimeException("stub"); } }
    public interface UsageEventListener { void onUsageEvent(int user, UsageEvents.Event event); }
    public interface EstimatedLaunchTimeChangedListener { void onEstimatedLaunchTimeChanged(int user, String name, long time); }
}
