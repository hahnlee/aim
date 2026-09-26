package dev.darwinart.runtime.am;

import android.app.ActivityManagerInternal;

/**
 * The activity manager's local interface, published in LocalServices as
 * AOSP's ActivityManagerService does, for the AOSP services this runtime runs
 * (PackageManagerService, AppOpsService and their owners). Each method is
 * answered from this runtime's activity manager state or reports that the
 * runtime does not provide it; none fabricates a result.
 */
public final class ActivityManagerLocal extends ActivityManagerInternal {
    private final BroadcastTransactions broadcasts;

    ActivityManagerLocal(BroadcastTransactions broadcasts) {
        this.broadcasts = broadcasts;
    }

    private static UnsupportedOperationException unsupported(String method) {
        return new UnsupportedOperationException(
                "ActivityManagerInternal." + method + " is not provided by this runtime");
    }

    @Override
    public void addAppBackgroundRestrictionListener(android.app.ActivityManagerInternal.AppBackgroundRestrictionListener arg0) {
        throw unsupported("addAppBackgroundRestrictionListener");
    }

    @Override
    public void addBindServiceEventListener(android.app.ActivityManagerInternal.BindServiceEventListener arg0) {
        throw unsupported("addBindServiceEventListener");
    }

    @Override
    public void addBroadcastEventListener(android.app.ActivityManagerInternal.BroadcastEventListener arg0) {
        throw unsupported("addBroadcastEventListener");
    }

    @Override
    public void addCreatorToken(android.content.Intent arg0, java.lang.String arg1) {
        throw unsupported("addCreatorToken");
    }

    @Override
    public void addForegroundServiceStateListener(android.app.ActivityManagerInternal.ForegroundServiceStateListener arg0) {
        throw unsupported("addForegroundServiceStateListener");
    }

    @Override
    public void addFrozenProcessListener(int arg0, java.util.concurrent.Executor arg1, android.app.ActivityManagerInternal.FrozenProcessListener arg2) {
        throw unsupported("addFrozenProcessListener");
    }

    @Override
    public void addPendingTopUid(int arg0, int arg1, android.app.IApplicationThread arg2) {
        throw unsupported("addPendingTopUid");
    }

    @Override
    public void addStartInfoTimestamp(int arg0, long arg1, int arg2, int arg3, int arg4) {
        throw unsupported("addStartInfoTimestamp");
    }

    @Override
    public void appNotResponding(java.lang.String arg0, int arg1, com.android.internal.os.TimeoutRecord arg2) {
        throw unsupported("appNotResponding");
    }

    @Override
    public android.app.ActivityManagerInternal.ServiceNotificationPolicy applyForegroundServiceNotification(android.app.Notification arg0, java.lang.String arg1, int arg2, java.lang.String arg3, int arg4) {
        throw unsupported("applyForegroundServiceNotification");
    }

    @Override
    public void broadcastCloseSystemDialogs(java.lang.String arg0) {
        throw unsupported("broadcastCloseSystemDialogs");
    }

    @Override
    public void broadcastGlobalConfigurationChanged(int arg0, boolean arg1) {
        throw unsupported("broadcastGlobalConfigurationChanged");
    }

    @Override
    public int broadcastIntent(android.content.Intent arg0, android.content.IIntentReceiver arg1, java.lang.String[] arg2, boolean arg3, int arg4, int[] arg5, java.util.function.BiFunction<java.lang.Integer, android.os.Bundle, android.os.Bundle> arg6, android.os.Bundle arg7) {
        throw unsupported("broadcastIntent");
    }

    @Override
    public int broadcastIntentInPackage(java.lang.String arg0, java.lang.String arg1, int arg2, int arg3, int arg4, android.content.Intent arg5, java.lang.String arg6, android.app.IApplicationThread arg7, android.content.IIntentReceiver arg8, int arg9, java.lang.String arg10, android.os.Bundle arg11, java.lang.String arg12, android.os.Bundle arg13, boolean arg14, boolean arg15, int arg16, android.app.BackgroundStartPrivileges arg17, int[] arg18) {
        throw unsupported("broadcastIntentInPackage");
    }

    @Override
    public int broadcastIntentWithCallback(android.content.Intent arg0, android.content.IIntentReceiver arg1, java.lang.String[] arg2, int arg3, int[] arg4, java.util.function.BiFunction<java.lang.Integer, android.os.Bundle, android.os.Bundle> arg5, android.os.Bundle arg6) {
        // arg6 (BroadcastOptions) selects delivery policy of the broadcast
        // queue, which delivers every broadcast immediately here.
        return broadcasts.broadcastWithCallback(arg0, arg1, arg2, arg3, arg4, arg5);
    }

    @Override
    public boolean canAllowWhileInUsePermissionInFgs(int arg0, int arg1, java.lang.String arg2) {
        throw unsupported("canAllowWhileInUsePermissionInFgs");
    }

    @Override
    public boolean canScheduleUserInitiatedJobs(int arg0, int arg1, java.lang.String arg2) {
        throw unsupported("canScheduleUserInitiatedJobs");
    }

    @Override
    public boolean canStartForegroundService(int arg0, int arg1, java.lang.String arg2) {
        throw unsupported("canStartForegroundService");
    }

    @Override
    public boolean canStartMoreUsers() {
        throw unsupported("canStartMoreUsers");
    }

    @Override
    public java.lang.String checkContentProviderAccess(java.lang.String arg0, int arg1) {
        throw unsupported("checkContentProviderAccess");
    }

    @Override
    public int checkContentProviderUriPermission(android.net.Uri arg0, int arg1, int arg2, int arg3) {
        throw unsupported("checkContentProviderUriPermission");
    }

    @Override
    public void cleanUpServices(int arg0, android.content.ComponentName arg1, android.content.Intent arg2) {
        throw unsupported("cleanUpServices");
    }

    @Override
    public boolean clearApplicationUserData(java.lang.String arg0, boolean arg1, boolean arg2, android.content.pm.IPackageDataObserver arg3, int arg4) {
        throw unsupported("clearApplicationUserData");
    }

    @Override
    public void clearPendingBackup(int arg0) {
        throw unsupported("clearPendingBackup");
    }

    @Override
    public void clearPendingIntentAllowBgActivityStarts(android.content.IIntentSender arg0, android.os.IBinder arg1) {
        throw unsupported("clearPendingIntentAllowBgActivityStarts");
    }

    @Override
    public void deletePendingTopUid(int arg0, long arg1) {
        throw unsupported("deletePendingTopUid");
    }

    @Override
    public void disconnectActivityFromServices(java.lang.Object arg0) {
        throw unsupported("disconnectActivityFromServices");
    }

    @Override
    public void enforceBroadcastOptionsPermissions(android.os.Bundle arg0, int arg1) {
        throw unsupported("enforceBroadcastOptionsPermissions");
    }

    @Override
    public void enforceCallingPermission(java.lang.String arg0, java.lang.String arg1) {
        throw unsupported("enforceCallingPermission");
    }

    @Override
    public void ensureBootCompleted() {
        throw unsupported("ensureBootCompleted");
    }

    @Override
    public void ensureNotSpecialUser(int arg0) {
        throw unsupported("ensureNotSpecialUser");
    }

    @Override
    public void finishBooting() {
        throw unsupported("finishBooting");
    }

    @Override
    public void finishUserSwitch(java.lang.Object arg0) {
        throw unsupported("finishUserSwitch");
    }

    @Override
    public android.content.pm.ActivityInfo getActivityInfoForUser(android.content.pm.ActivityInfo arg0, int arg1) {
        throw unsupported("getActivityInfoForUser");
    }

    @Override
    public android.content.pm.ActivityPresentationInfo getActivityPresentationInfo(android.os.IBinder arg0) {
        throw unsupported("getActivityPresentationInfo");
    }

    @Override
    public android.util.Pair<java.lang.String, java.lang.String> getAppProfileStatsForDebugging(long arg0, int arg1) {
        throw unsupported("getAppProfileStatsForDebugging");
    }

    @Override
    public android.app.BackgroundStartPrivileges getBackgroundStartPrivileges(int arg0) {
        throw unsupported("getBackgroundStartPrivileges");
    }

    /**
     * ActivityManagerConstants.DEFAULT_BOOT_TIME_TEMP_ALLOWLIST_DURATION: the
     * temp-allowlist window PMS attaches to package broadcasts. No DeviceConfig
     * override exists in this runtime.
     */
    @Override
    public long getBootTimeTempAllowListDuration() {
        return 20 * 1000;
    }

    @Override
    public java.lang.Object getCachedAppsHighWatermarkStats(int arg0, boolean arg1) {
        throw unsupported("getCachedAppsHighWatermarkStats");
    }

    @Override
    public android.util.ArraySet<java.lang.String> getClientPackages(java.lang.String arg0) {
        throw unsupported("getClientPackages");
    }

    @Override
    public android.util.Pair<java.lang.Integer, java.lang.Integer> getCurrentAndTargetUserIds() {
        throw unsupported("getCurrentAndTargetUserIds");
    }

    @Override
    public int[] getCurrentProfileIds() {
        throw unsupported("getCurrentProfileIds");
    }

    @Override
    public android.content.pm.UserInfo getCurrentUser() {
        throw unsupported("getCurrentUser");
    }

    @Override
    public int getCurrentUserId() {
        throw unsupported("getCurrentUserId");
    }

    @Override
    public void getExecutableMethodFileOffsets(java.lang.String arg0, int arg1, int arg2, android.os.instrumentation.MethodDescriptor arg3, android.os.instrumentation.IOffsetCallback arg4) {
        throw unsupported("getExecutableMethodFileOffsets");
    }

    @Override
    public int getInstrumentationSourceUid(int arg0) {
        // Instrumentation is not supported, so no uid is instrumented.
        return android.os.Process.INVALID_UID;
    }

    @Override
    public android.content.Intent getIntentForIntentSender(android.content.IIntentSender arg0) {
        throw unsupported("getIntentForIntentSender");
    }

    @Override
    public java.util.List<java.lang.Integer> getIsolatedProcesses(int arg0) {
        throw unsupported("getIsolatedProcesses");
    }

    @Override
    public int getMaxRunningUsers() {
        throw unsupported("getMaxRunningUsers");
    }

    @Override
    public java.util.List<android.app.ProcessMemoryState> getMemoryStateForProcesses() {
        throw unsupported("getMemoryStateForProcesses");
    }

    @Override
    public java.lang.String getPackageNameByPid(int arg0) {
        throw unsupported("getPackageNameByPid");
    }

    @Override
    public android.app.PendingIntent getPendingIntentActivityAsApp(int arg0, android.content.Intent arg1, int arg2, android.os.Bundle arg3, java.lang.String arg4, int arg5) {
        throw unsupported("getPendingIntentActivityAsApp");
    }

    @Override
    public android.app.PendingIntent getPendingIntentActivityAsApp(int arg0, android.content.Intent[] arg1, int arg2, android.os.Bundle arg3, java.lang.String arg4, int arg5) {
        throw unsupported("getPendingIntentActivityAsApp");
    }

    @Override
    public int getPendingIntentFlags(android.content.IIntentSender arg0) {
        throw unsupported("getPendingIntentFlags");
    }

    @Override
    public java.util.List<android.app.PendingIntentStats> getPendingIntentStats() {
        throw unsupported("getPendingIntentStats");
    }

    @Override
    public java.util.Map<java.lang.Integer, java.lang.String> getProcessesWithPendingBindMounts(int arg0) {
        throw unsupported("getProcessesWithPendingBindMounts");
    }

    @Override
    public int getPushMessagingOverQuotaBehavior() {
        throw unsupported("getPushMessagingOverQuotaBehavior");
    }

    @Override
    public int getRestrictionLevel(int arg0) {
        throw unsupported("getRestrictionLevel");
    }

    @Override
    public int getRestrictionLevel(java.lang.String arg0, int arg1) {
        throw unsupported("getRestrictionLevel");
    }

    @Override
    public int getServiceStartForegroundTimeout() {
        throw unsupported("getServiceStartForegroundTimeout");
    }

    @Override
    public int[] getStartedUserIds() {
        throw unsupported("getStartedUserIds");
    }

    @Override
    public int getStorageMountMode(int arg0, int arg1) {
        throw unsupported("getStorageMountMode");
    }

    @Override
    public int getTaskIdForActivity(android.os.IBinder arg0, boolean arg1) {
        throw unsupported("getTaskIdForActivity");
    }

    @Override
    public int getUidCapability(int arg0) {
        throw unsupported("getUidCapability");
    }

    @Override
    public int getUidProcessState(int arg0) {
        throw unsupported("getUidProcessState");
    }

    @Override
    public int handleIncomingUser(int arg0, int arg1, int arg2, boolean arg3, int arg4, java.lang.String arg5, java.lang.String arg6) {
        throw unsupported("handleIncomingUser");
    }

    @Override
    public boolean hasForegroundServiceNotification(java.lang.String arg0, int arg1, java.lang.String arg2) {
        throw unsupported("hasForegroundServiceNotification");
    }

    @Override
    public boolean hasRunningActivity(int arg0, java.lang.String arg1) {
        throw unsupported("hasRunningActivity");
    }

    @Override
    public boolean hasRunningForegroundService(int arg0, int arg1) {
        throw unsupported("hasRunningForegroundService");
    }

    @Override
    public boolean hasStartedUserState(int arg0) {
        throw unsupported("hasStartedUserState");
    }

    @Override
    public void inputDispatchingResumed(int arg0) {
        throw unsupported("inputDispatchingResumed");
    }

    @Override
    public long inputDispatchingTimedOut(int arg0, boolean arg1, com.android.internal.os.TimeoutRecord arg2) {
        throw unsupported("inputDispatchingTimedOut");
    }

    @Override
    public boolean inputDispatchingTimedOut(java.lang.Object arg0, java.lang.String arg1, android.content.pm.ApplicationInfo arg2, java.lang.String arg3, java.lang.Object arg4, boolean arg5, com.android.internal.os.TimeoutRecord arg6) {
        throw unsupported("inputDispatchingTimedOut");
    }

    @Override
    public boolean isActivityStartsLoggingEnabled() {
        throw unsupported("isActivityStartsLoggingEnabled");
    }

    @Override
    public boolean isAppBad(java.lang.String arg0, int arg1) {
        throw unsupported("isAppBad");
    }

    @Override
    public boolean isAppForeground(int arg0) {
        throw unsupported("isAppForeground");
    }

    @Override
    public boolean isAppStartModeDisabled(int arg0, java.lang.String arg1) {
        throw unsupported("isAppStartModeDisabled");
    }

    @Override
    public boolean isAssociatedCompanionApp(int arg0, int arg1) {
        throw unsupported("isAssociatedCompanionApp");
    }

    @Override
    public boolean isBackgroundActivityStartsEnabled() {
        throw unsupported("isBackgroundActivityStartsEnabled");
    }

    @Override
    public boolean isBgAutoRestrictedBucketFeatureFlagEnabled() {
        throw unsupported("isBgAutoRestrictedBucketFeatureFlagEnabled");
    }

    @Override
    public boolean isBooted() {
        throw unsupported("isBooted");
    }

    @Override
    public boolean isBooting() {
        throw unsupported("isBooting");
    }

    @Override
    public boolean isCurrentProfile(int arg0) {
        throw unsupported("isCurrentProfile");
    }

    @Override
    public boolean isDeviceOwner(int arg0) {
        throw unsupported("isDeviceOwner");
    }

    @Override
    public boolean isEarlyPackageKillEnabledForUserSwitch(int arg0, int arg1) {
        throw unsupported("isEarlyPackageKillEnabledForUserSwitch");
    }

    @Override
    public boolean isPendingTopUid(int arg0) {
        // Activities start synchronously here: no uid waits to become top.
        return false;
    }

    @Override
    public boolean isProfileOwner(int arg0) {
        throw unsupported("isProfileOwner");
    }

    @Override
    public boolean isRuntimeRestarted() {
        throw unsupported("isRuntimeRestarted");
    }

    @Override
    public boolean isSystemReady() {
        throw unsupported("isSystemReady");
    }

    @Override
    public boolean isTempAllowlistedForFgsWhileInUse(int arg0) {
        // This runtime keeps no temporary allowlist (nothing adds to one).
        return false;
    }

    @Override
    public boolean isUidActive(int arg0) {
        throw unsupported("isUidActive");
    }

    @Override
    public boolean isUserRunning(int arg0, int arg1) {
        throw unsupported("isUserRunning");
    }

    @Override
    public void killAllBackgroundProcessesExcept(int arg0, int arg1) {
        throw unsupported("killAllBackgroundProcessesExcept");
    }

    @Override
    public void killApplicationSync(java.lang.String arg0, int arg1, int arg2, java.lang.String arg3, int arg4) {
        throw unsupported("killApplicationSync");
    }

    @Override
    public void killForegroundAppsForUser(int arg0) {
        throw unsupported("killForegroundAppsForUser");
    }

    @Override
    public void killProcess(java.lang.String arg0, int arg1, java.lang.String arg2) {
        throw unsupported("killProcess");
    }

    @Override
    public void killProcessesForRemovedTask(java.util.ArrayList<java.lang.Object> arg0) {
        throw unsupported("killProcessesForRemovedTask");
    }

    @Override
    public void logFgsApiBegin(int arg0, int arg1, int arg2) {
        throw unsupported("logFgsApiBegin");
    }

    @Override
    public void logFgsApiEnd(int arg0, int arg1, int arg2) {
        throw unsupported("logFgsApiEnd");
    }

    @Override
    public void monitor() {
        throw unsupported("monitor");
    }

    @Override
    public void noteAlarmFinish(android.app.PendingIntent arg0, android.os.WorkSource arg1, int arg2, java.lang.String arg3) {
        throw unsupported("noteAlarmFinish");
    }

    @Override
    public void noteAlarmStart(android.app.PendingIntent arg0, android.os.WorkSource arg1, int arg2, java.lang.String arg3) {
        throw unsupported("noteAlarmStart");
    }

    @Override
    public void noteWakeupAlarm(android.app.PendingIntent arg0, android.os.WorkSource arg1, int arg2, java.lang.String arg3, java.lang.String arg4) {
        throw unsupported("noteWakeupAlarm");
    }

    @Override
    public void notifyActiveMediaForegroundService(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("notifyActiveMediaForegroundService");
    }

    @Override
    public void notifyInactiveMediaForegroundService(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("notifyInactiveMediaForegroundService");
    }

    @Override
    public void notifyMediaProjectionEvent(int arg0, android.os.IBinder arg1, int arg2) {
        throw unsupported("notifyMediaProjectionEvent");
    }

    @Override
    public void notifyNetworkPolicyRulesUpdated(int arg0, long arg1) {
        throw unsupported("notifyNetworkPolicyRulesUpdated");
    }

    @Override
    public void onForegroundServiceNotificationUpdate(boolean arg0, android.app.Notification arg1, int arg2, java.lang.String arg3, int arg4) {
        throw unsupported("onForegroundServiceNotificationUpdate");
    }

    @Override
    public void onUidBlockedReasonsChanged(int arg0, int arg1) {
        throw unsupported("onUidBlockedReasonsChanged");
    }

    @Override
    public void onUserRemoved(int arg0) {
        throw unsupported("onUserRemoved");
    }

    @Override
    public void onUserRemoving(int arg0) {
        throw unsupported("onUserRemoving");
    }

    @Override
    public void onWakefulnessChanged(int arg0) {
        throw unsupported("onWakefulnessChanged");
    }

    @Override
    public void prepareForPossibleShutdown() {
        throw unsupported("prepareForPossibleShutdown");
    }

    @Override
    public void registerAnrController(android.app.AnrController arg0) {
        throw unsupported("registerAnrController");
    }

    @Override
    public void registerNetworkPolicyUidObserver(android.app.IUidObserver arg0, int arg1, int arg2, java.lang.String arg3) {
        throw unsupported("registerNetworkPolicyUidObserver");
    }

    @Override
    public void registerProcessObserver(android.app.IProcessObserver arg0) {
        throw unsupported("registerProcessObserver");
    }

    @Override
    public void reportCurKeyguardUsageEvent(boolean arg0) {
        throw unsupported("reportCurKeyguardUsageEvent");
    }

    @Override
    public void rescheduleAnrDialog(java.lang.Object arg0) {
        throw unsupported("rescheduleAnrDialog");
    }

    @Override
    public void restart() {
        throw unsupported("restart");
    }

    @Override
    public void scheduleAppGcs() {
        throw unsupported("scheduleAppGcs");
    }

    @Override
    public void sendForegroundProfileChanged(int arg0) {
        throw unsupported("sendForegroundProfileChanged");
    }

    @Override
    public int sendIntentSender(android.content.IIntentSender arg0, android.os.IBinder arg1, int arg2, android.content.Intent arg3, java.lang.String arg4, android.content.IIntentReceiver arg5, java.lang.String arg6, android.os.Bundle arg7) {
        throw unsupported("sendIntentSender");
    }

    @Override
    public void setBooted(boolean arg0) {
        throw unsupported("setBooted");
    }

    @Override
    public void setBooting(boolean arg0) {
        throw unsupported("setBooting");
    }

    @Override
    public void setCompanionAppUids(int arg0, java.util.Set<java.lang.Integer> arg1) {
        throw unsupported("setCompanionAppUids");
    }

    @Override
    public void setDebugFlagsForStartingActivity(android.content.pm.ActivityInfo arg0, int arg1, android.app.ProfilerInfo arg2, java.lang.Object arg3) {
        throw unsupported("setDebugFlagsForStartingActivity");
    }

    @Override
    public void setDeviceIdleAllowlist(int[] arg0, int[] arg1) {
        throw unsupported("setDeviceIdleAllowlist");
    }

    @Override
    public void setDeviceOwnerUid(int arg0) {
        throw unsupported("setDeviceOwnerUid");
    }

    @Override
    public void setHasOverlayUi(int arg0, boolean arg1) {
        throw unsupported("setHasOverlayUi");
    }

    @Override
    public void setPendingIntentAllowBgActivityStarts(android.content.IIntentSender arg0, android.os.IBinder arg1, int arg2) {
        throw unsupported("setPendingIntentAllowBgActivityStarts");
    }

    @Override
    public void setPendingIntentAllowlistDuration(android.content.IIntentSender arg0, android.os.IBinder arg1, long arg2, int arg3, int arg4, java.lang.String arg5) {
        throw unsupported("setPendingIntentAllowlistDuration");
    }

    @Override
    public void setProfileOwnerUid(android.util.ArraySet<java.lang.Integer> arg0) {
        throw unsupported("setProfileOwnerUid");
    }

    @Override
    public void setStopUserOnSwitch(int arg0) {
        throw unsupported("setStopUserOnSwitch");
    }

    @Override
    public void setSwitchingFromUserMessage(int arg0, java.lang.String arg1) {
        throw unsupported("setSwitchingFromUserMessage");
    }

    @Override
    public void setSwitchingToUserMessage(int arg0, java.lang.String arg1) {
        throw unsupported("setSwitchingToUserMessage");
    }

    @Override
    public void setVoiceInteractionManagerProvider(android.app.ActivityManagerInternal.VoiceInteractionManagerProvider arg0) {
        throw unsupported("setVoiceInteractionManagerProvider");
    }

    @Override
    public boolean shouldConfirmCredentials(int arg0) {
        throw unsupported("shouldConfirmCredentials");
    }

    @Override
    public boolean shouldDelayHomeLaunch(int arg0) {
        throw unsupported("shouldDelayHomeLaunch");
    }

    @Override
    public boolean startForegroundServiceDelegate(android.app.ForegroundServiceDelegationOptions arg0, android.content.ServiceConnection arg1) {
        throw unsupported("startForegroundServiceDelegate");
    }

    @Override
    public boolean startIsolatedProcess(java.lang.String arg0, java.lang.String[] arg1, java.lang.String arg2, java.lang.String arg3, int arg4, java.lang.Runnable arg5) {
        throw unsupported("startIsolatedProcess");
    }

    @Override
    public void startProcess(java.lang.String arg0, android.content.pm.ApplicationInfo arg1, boolean arg2, boolean arg3, java.lang.String arg4, android.content.ComponentName arg5) {
        throw unsupported("startProcess");
    }

    @Override
    public boolean startProfileEvenWhenDisabled(int arg0) {
        throw unsupported("startProfileEvenWhenDisabled");
    }

    @Override
    public android.content.ComponentName startServiceInPackage(int arg0, android.content.Intent arg1, java.lang.String arg2, boolean arg3, java.lang.String arg4, java.lang.String arg5, int arg6, android.app.BackgroundStartPrivileges arg7) throws android.os.TransactionTooLargeException {
        throw unsupported("startServiceInPackage");
    }

    @Override
    public boolean startUserInBackground(int arg0) {
        throw unsupported("startUserInBackground");
    }

    @Override
    public void stopAppForUser(java.lang.String arg0, int arg1) {
        throw unsupported("stopAppForUser");
    }

    @Override
    public void stopForegroundServiceDelegate(android.app.ForegroundServiceDelegationOptions arg0) {
        throw unsupported("stopForegroundServiceDelegate");
    }

    @Override
    public void stopForegroundServiceDelegate(android.content.ServiceConnection arg0) {
        throw unsupported("stopForegroundServiceDelegate");
    }

    @Override
    public void tempAllowWhileInUsePermissionInFgs(int arg0, long arg1) {
        throw unsupported("tempAllowWhileInUsePermissionInFgs");
    }

    @Override
    public void tempAllowlistForPendingIntent(int arg0, int arg1, int arg2, long arg3, int arg4, int arg5, java.lang.String arg6) {
        throw unsupported("tempAllowlistForPendingIntent");
    }

    @Override
    public void triggerUnsafeIntentStrictMode(int arg0, int arg1, android.content.Intent arg2) {
        throw unsupported("triggerUnsafeIntentStrictMode");
    }

    @Override
    public void trimApplications() {
        throw unsupported("trimApplications");
    }

    @Override
    public void unregisterAnrController(android.app.AnrController arg0) {
        throw unsupported("unregisterAnrController");
    }

    @Override
    public void unregisterProcessObserver(android.app.IProcessObserver arg0) {
        throw unsupported("unregisterProcessObserver");
    }

    @Override
    public void updateActivityUsageStats(android.content.ComponentName arg0, int arg1, int arg2, android.os.IBinder arg3, android.content.ComponentName arg4, android.app.assist.ActivityId arg5) {
        throw unsupported("updateActivityUsageStats");
    }

    @Override
    public void updateBatteryStats(android.content.ComponentName arg0, int arg1, int arg2, boolean arg3) {
        throw unsupported("updateBatteryStats");
    }

    @Override
    public void updateCpuStats() {
        throw unsupported("updateCpuStats");
    }

    @Override
    public void updateDeviceIdleTempAllowlist(int[] arg0, int arg1, boolean arg2, long arg3, int arg4, int arg5, java.lang.String arg6, int arg7) {
        throw unsupported("updateDeviceIdleTempAllowlist");
    }

    @Override
    public void updateForegroundTimeIfOnBattery(java.lang.String arg0, int arg1, long arg2) {
        throw unsupported("updateForegroundTimeIfOnBattery");
    }

    @Override
    public void updateOomAdj(int arg0) {
        throw unsupported("updateOomAdj");
    }

    @Override
    public void updateOomLevelsForDisplay(int arg0) {
        throw unsupported("updateOomLevelsForDisplay");
    }
}
