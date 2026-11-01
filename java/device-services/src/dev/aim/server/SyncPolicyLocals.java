package dev.aim.server;

import android.app.usage.UsageEvents;
import android.app.usage.UsageStats;
import android.app.usage.UsageStatsManagerInternal;
import android.content.ComponentName;
import android.content.LocusId;
import android.content.res.Configuration;
import android.os.IBinder;
import android.os.PersistableBundle;
import android.os.UserHandle;
import java.util.List;
import java.util.Set;
import android.os.RemoteException;
import com.android.server.DeviceIdleInternal;
import com.android.server.LocalServices;
import com.android.server.deviceidle.IDeviceIdleConstraint;
import java.util.Objects;

/** Local sync policy views; the native owners retain all state (#1460). */
final class SyncPolicyLocals {
    static void publish(ISystemProcess owner) {
        LocalServices.addService(UsageStatsManagerInternal.class, new Usage(owner));
        LocalServices.addService(DeviceIdleInternal.class, new Idle(owner));
    }
    static final class Usage extends UsageStatsManagerInternal {
        private final ISystemProcess owner;
        Usage(ISystemProcess owner) { this.owner = Objects.requireNonNull(owner); }
        private UnsupportedOperationException unsupported() {
            return new UnsupportedOperationException("UsageStatsManagerInternal operation (#1723)");
        }
        @Override public void reportEvent(ComponentName component, int userId, int eventType, int instanceId, ComponentName taskRoot) { throw unsupported(); }
        @Override public void reportEvent(String packageName, int userId, int eventType) { throw unsupported(); }
        @Override public void reportEventForAllUsers(String packageName, int eventType) { throw unsupported(); }
        @Override public void reportConfigurationChange(Configuration config, int userId) { throw unsupported(); }
        @Override public void reportInterruptiveNotification(String packageName, String channelId, int userId) { throw unsupported(); }
        @Override public void reportShortcutUsage(String packageName, String shortcutId, int userId) { throw unsupported(); }
        @Override public void reportContentProviderUsage(String name, String pkgName, int userId) { throw unsupported(); }
        @Override public void reportLocusUpdate(ComponentName activity, int userId, LocusId locusId, IBinder appToken) { throw unsupported(); }
        @Override public void reportUserInteractionEvent(String pkgName, int userId, PersistableBundle extras) { throw unsupported(); }
        @Override public void prepareShutdown() { throw unsupported(); }
        @Override public void prepareForPossibleShutdown() { throw unsupported(); }
        @Override public boolean isAppIdle(String packageName, int uidForAppId, int userId) { throw unsupported(); }
        @Override public int getAppStandbyBucket(String packageName, int userId, long nowElapsed) { try { return owner.getSyncAdapterStandbyBucket(packageName, userId); } catch (RemoteException e) { throw e.rethrowFromSystemServer(); } }
        @Override public int[] getIdleUidsForUser(int userId) { throw unsupported(); }
        @Override public byte[] getBackupPayload(int userId, String key) { throw unsupported(); }
        @Override public void applyRestoredPayload(int userId, String key, byte[] payload) { throw unsupported(); }
        @Override public void onActiveAdminAdded(String packageName, int userId) { throw unsupported(); }
        @Override public void setActiveAdminApps(Set<String> adminApps, int userId) { throw unsupported(); }
        @Override public void setAdminProtectedPackages(Set<String> packageNames, int userId) { throw unsupported(); }
        @Override public void onAdminDataAvailable() { throw unsupported(); }
        @Override public List<UsageStats> queryUsageStatsForUser(int userId, int interval, long beginTime, long endTime, boolean obfuscateInstantApps) { throw unsupported(); }
        @Override public UsageEvents queryEventsForUser(int userId, long beginTime, long endTime, int flags) { throw unsupported(); }
        @Override public void setLastJobRunTime(String packageName, int userId, long elapsedRealtime) { throw unsupported(); }
        @Override public long getEstimatedPackageLaunchTime(String packageName, int userId) { throw unsupported(); }
        @Override public long getTimeSinceLastJobRun(String packageName, int userId) { throw unsupported(); }
        @Override public void reportSyncScheduled(String packageName, int userId, boolean exempted) { try { owner.reportSyncScheduled(packageName, userId, exempted); } catch (RemoteException e) { throw e.rethrowFromSystemServer(); } }
        @Override public void reportExemptedSyncStart(String packageName, int userId) { try { owner.reportExemptedSyncStart(packageName, userId); } catch (RemoteException e) { throw e.rethrowFromSystemServer(); } }
        @Override public AppUsageLimitData getAppUsageLimit(String packageName, UserHandle user) { throw unsupported(); }
        @Override public boolean pruneUninstalledPackagesData(int userId) { throw unsupported(); }
        @Override public boolean updatePackageMappingsData(int userId) { throw unsupported(); }
        @Override public void registerListener(UsageEventListener listener) { throw unsupported(); }
        @Override public void unregisterListener(UsageEventListener listener) { throw unsupported(); }
        @Override public void registerLaunchTimeChangedListener( EstimatedLaunchTimeChangedListener listener) { throw unsupported(); }
        @Override public void reportBroadcastDispatched(int sourceUid, String targetPackage, UserHandle targetUser, long idForResponseEvent, long timestampMs, int targetUidProcState) { throw unsupported(); }
        @Override public void reportNotificationPosted(String packageName, UserHandle user, long timestampMs) { throw unsupported(); }
        @Override public void reportNotificationUpdated(String packageName, UserHandle user, long timestampMs) { throw unsupported(); }
        @Override public void reportNotificationRemoved(String packageName, UserHandle user, long timestampMs) { throw unsupported(); }
    }
    static final class Idle implements DeviceIdleInternal {
        private final ISystemProcess owner;
        Idle(ISystemProcess owner) { this.owner = Objects.requireNonNull(owner); }
        private UnsupportedOperationException unsupported() {
            return new UnsupportedOperationException("DeviceIdleInternal operation (#2016)");
        }
        @Override public void onConstraintStateChanged(IDeviceIdleConstraint constraint, boolean active) { throw unsupported(); }
        @Override public void registerDeviceIdleConstraint(IDeviceIdleConstraint constraint, String name, int minState) { throw unsupported(); }
        @Override public void unregisterDeviceIdleConstraint(IDeviceIdleConstraint constraint) { throw unsupported(); }
        @Override public void exitIdle(String reason) { throw unsupported(); }
        @Override public void addPowerSaveTempWhitelistApp(int callingUid, String packageName, long durationMs, int userId, boolean sync, int reasonCode, String reason) { addPowerSaveTempWhitelistApp(callingUid, packageName, durationMs, 0, userId, sync, reasonCode, reason); }
        @Override public void addPowerSaveTempWhitelistApp(int callingUid, String packageName, long durationMs, int tempAllowListType, int userId, boolean sync, int reasonCode, String reason) { try { owner.addSyncTempAllowlist(callingUid, packageName, durationMs, tempAllowListType, userId, sync, reasonCode, reason); } catch (RemoteException e) { throw e.rethrowFromSystemServer(); } }
        @Override public void addPowerSaveTempWhitelistAppDirect(int uid, long duration, int type, boolean sync, int reasonCode, String reason, int callingUid) { throw unsupported(); }
        @Override public long getNotificationAllowlistDuration() { throw unsupported(); }
        @Override public void setJobsActive(boolean active) { throw unsupported(); }
        @Override public void setAlarmsActive(boolean active) { throw unsupported(); }
        @Override public boolean isAppOnWhitelist(int appid) { throw unsupported(); }
        @Override public int[] getPowerSaveWhitelistUserAppIds() { throw unsupported(); }
        @Override public int[] getPowerSaveTempWhitelistAppIds() { throw unsupported(); }
        @Override public String[] getFullPowerWhitelistExceptIdle() { throw unsupported(); }
        @Override public void registerStationaryListener(StationaryListener listener) { throw unsupported(); }
        @Override public void unregisterStationaryListener(StationaryListener listener) { throw unsupported(); }
        @Override public int getTempAllowListType(int reasonCode, int defaultType) { throw unsupported(); }
    }
}
