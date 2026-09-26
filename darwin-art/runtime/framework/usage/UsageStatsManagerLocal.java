package dev.darwinart.runtime.usage;

import android.app.usage.UsageStatsManagerInternal;
import java.util.Set;
import java.util.concurrent.CopyOnWriteArraySet;

/**
 * The usage-stats local interface, published in LocalServices as AOSP's
 * UsageStatsService does, for the AOSP services this runtime runs. This
 * runtime records no usage events, so listeners are registered but receive
 * none; every query reports that the runtime does not provide it.
 */
public final class UsageStatsManagerLocal extends UsageStatsManagerInternal {
    private final Set<UsageEventListener> listeners = new CopyOnWriteArraySet<>();

    private static UnsupportedOperationException unsupported(String method) {
        return new UnsupportedOperationException(
                "UsageStatsManagerInternal." + method + " is not provided by this runtime");
    }

    @Override
    public void applyRestoredPayload(int arg0, java.lang.String arg1, byte[] arg2) {
        throw unsupported("applyRestoredPayload");
    }

    @Override
    public int getAppStandbyBucket(java.lang.String arg0, int arg1, long arg2) {
        throw unsupported("getAppStandbyBucket");
    }

    @Override
    public android.app.usage.UsageStatsManagerInternal.AppUsageLimitData getAppUsageLimit(java.lang.String arg0, android.os.UserHandle arg1) {
        throw unsupported("getAppUsageLimit");
    }

    @Override
    public byte[] getBackupPayload(int arg0, java.lang.String arg1) {
        throw unsupported("getBackupPayload");
    }

    @Override
    public long getEstimatedPackageLaunchTime(java.lang.String arg0, int arg1) {
        throw unsupported("getEstimatedPackageLaunchTime");
    }

    @Override
    public int[] getIdleUidsForUser(int arg0) {
        throw unsupported("getIdleUidsForUser");
    }

    @Override
    public long getTimeSinceLastJobRun(java.lang.String arg0, int arg1) {
        throw unsupported("getTimeSinceLastJobRun");
    }

    @Override
    public boolean isAppIdle(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("isAppIdle");
    }

    @Override
    public void onActiveAdminAdded(java.lang.String arg0, int arg1) {
        throw unsupported("onActiveAdminAdded");
    }

    @Override
    public void onAdminDataAvailable() {
        throw unsupported("onAdminDataAvailable");
    }

    @Override
    public void prepareForPossibleShutdown() {
        throw unsupported("prepareForPossibleShutdown");
    }

    @Override
    public void prepareShutdown() {
        throw unsupported("prepareShutdown");
    }

    @Override
    public boolean pruneUninstalledPackagesData(int arg0) {
        throw unsupported("pruneUninstalledPackagesData");
    }

    @Override
    public android.app.usage.UsageEvents queryEventsForUser(int arg0, long arg1, long arg2, int arg3) {
        throw unsupported("queryEventsForUser");
    }

    @Override
    public java.util.List queryUsageStatsForUser(int arg0, int arg1, long arg2, long arg3, boolean arg4) {
        throw unsupported("queryUsageStatsForUser");
    }

    @Override
    public void registerLaunchTimeChangedListener(android.app.usage.UsageStatsManagerInternal.EstimatedLaunchTimeChangedListener arg0) {
        throw unsupported("registerLaunchTimeChangedListener");
    }

    @Override
    public void registerListener(android.app.usage.UsageStatsManagerInternal.UsageEventListener arg0) {
        listeners.add(java.util.Objects.requireNonNull(arg0));
    }

    @Override
    public void reportBroadcastDispatched(int arg0, java.lang.String arg1, android.os.UserHandle arg2, long arg3, long arg4, int arg5) {
        throw unsupported("reportBroadcastDispatched");
    }

    @Override
    public void reportConfigurationChange(android.content.res.Configuration arg0, int arg1) {
        throw unsupported("reportConfigurationChange");
    }

    @Override
    public void reportContentProviderUsage(java.lang.String arg0, java.lang.String arg1, int arg2) {
        throw unsupported("reportContentProviderUsage");
    }

    @Override
    public void reportEvent(android.content.ComponentName arg0, int arg1, int arg2, int arg3, android.content.ComponentName arg4) {
        throw unsupported("reportEvent");
    }

    @Override
    public void reportEvent(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("reportEvent");
    }

    @Override
    public void reportEventForAllUsers(java.lang.String arg0, int arg1) {
        throw unsupported("reportEventForAllUsers");
    }

    @Override
    public void reportExemptedSyncStart(java.lang.String arg0, int arg1) {
        throw unsupported("reportExemptedSyncStart");
    }

    @Override
    public void reportInterruptiveNotification(java.lang.String arg0, java.lang.String arg1, int arg2) {
        throw unsupported("reportInterruptiveNotification");
    }

    @Override
    public void reportLocusUpdate(android.content.ComponentName arg0, int arg1, android.content.LocusId arg2, android.os.IBinder arg3) {
        throw unsupported("reportLocusUpdate");
    }

    @Override
    public void reportNotificationPosted(java.lang.String arg0, android.os.UserHandle arg1, long arg2) {
        throw unsupported("reportNotificationPosted");
    }

    @Override
    public void reportNotificationRemoved(java.lang.String arg0, android.os.UserHandle arg1, long arg2) {
        throw unsupported("reportNotificationRemoved");
    }

    @Override
    public void reportNotificationUpdated(java.lang.String arg0, android.os.UserHandle arg1, long arg2) {
        throw unsupported("reportNotificationUpdated");
    }

    @Override
    public void reportShortcutUsage(java.lang.String arg0, java.lang.String arg1, int arg2) {
        throw unsupported("reportShortcutUsage");
    }

    @Override
    public void reportSyncScheduled(java.lang.String arg0, int arg1, boolean arg2) {
        throw unsupported("reportSyncScheduled");
    }

    @Override
    public void reportUserInteractionEvent(java.lang.String arg0, int arg1, android.os.PersistableBundle arg2) {
        throw unsupported("reportUserInteractionEvent");
    }

    @Override
    public void setActiveAdminApps(java.util.Set arg0, int arg1) {
        throw unsupported("setActiveAdminApps");
    }

    @Override
    public void setAdminProtectedPackages(java.util.Set arg0, int arg1) {
        throw unsupported("setAdminProtectedPackages");
    }

    @Override
    public void setLastJobRunTime(java.lang.String arg0, int arg1, long arg2) {
        throw unsupported("setLastJobRunTime");
    }

    @Override
    public void unregisterListener(android.app.usage.UsageStatsManagerInternal.UsageEventListener arg0) {
        listeners.remove(arg0);
    }

    @Override
    public boolean updatePackageMappingsData(int arg0) {
        throw unsupported("updatePackageMappingsData");
    }
}
