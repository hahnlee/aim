// Compile-only android-16.0.0_r1 contract; checked against the image.
package com.android.server;
import com.android.server.deviceidle.IDeviceIdleConstraint;
public interface DeviceIdleInternal {
    void onConstraintStateChanged(IDeviceIdleConstraint constraint, boolean active);
    void registerDeviceIdleConstraint(IDeviceIdleConstraint constraint, String name, int minState);
    void unregisterDeviceIdleConstraint(IDeviceIdleConstraint constraint);
    void exitIdle(String reason);
    void addPowerSaveTempWhitelistApp(int callingUid, String packageName, long durationMs, int userId, boolean sync, int reasonCode, String reason);
    void addPowerSaveTempWhitelistApp(int callingUid, String packageName, long durationMs, int tempAllowListType, int userId, boolean sync, int reasonCode, String reason);
    void addPowerSaveTempWhitelistAppDirect(int uid, long duration, int type, boolean sync, int reasonCode, String reason, int callingUid);
    long getNotificationAllowlistDuration();
    void setJobsActive(boolean active);
    void setAlarmsActive(boolean active);
    boolean isAppOnWhitelist(int appid);
    int[] getPowerSaveWhitelistUserAppIds();
    int[] getPowerSaveTempWhitelistAppIds();
    String[] getFullPowerWhitelistExceptIdle();
    void registerStationaryListener(StationaryListener listener);
    void unregisterStationaryListener(StationaryListener listener);
    int getTempAllowListType(int reasonCode, int defaultType);
    interface StationaryListener { void onDeviceStationaryChanged(boolean stationary); }
}
