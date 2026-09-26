package dev.darwinart.runtime.wm;

import android.content.ComponentName;
import android.content.pm.PackageManagerInternal;
import android.content.res.Resources;
import android.os.Process;
import android.os.UserHandle;
import com.android.server.LocalServices;
import com.android.server.wm.ActivityTaskManagerInternal;

/**
 * The activity task manager's local interface, published in LocalServices
 * as AOSP's ActivityTaskManagerService does, for the AOSP services this
 * runtime runs (PackageManagerService and its owners). Each method is
 * answered from this runtime's task state or reports that the runtime does
 * not provide it; none fabricates a result.
 */
public final class ActivityTaskManagerLocal extends ActivityTaskManagerInternal {
    private static UnsupportedOperationException unsupported(String method) {
        return new UnsupportedOperationException(
                "ActivityTaskManagerInternal." + method + " is not provided by this runtime");
    }

    @Override
    public boolean attachApplication(com.android.server.wm.WindowProcessController arg0) {
        throw unsupported("attachApplication");
    }

    @Override
    public boolean canCloseSystemDialogs(int arg0, int arg1) {
        throw unsupported("canCloseSystemDialogs");
    }

    @Override
    public boolean canGcNow() {
        throw unsupported("canGcNow");
    }

    @Override
    public boolean canShowErrorDialogs(int arg0) {
        throw unsupported("canShowErrorDialogs");
    }

    @Override
    public boolean checkCanCloseSystemDialogs(int arg0, int arg1, java.lang.String arg2) {
        throw unsupported("checkCanCloseSystemDialogs");
    }

    @Override
    public void cleanupDisabledPackageComponents(java.lang.String arg0, java.util.Set arg1, int arg2, boolean arg3) {
        throw unsupported("cleanupDisabledPackageComponents");
    }

    @Override
    public void cleanupRecentTasksForUser(int arg0) {
        throw unsupported("cleanupRecentTasksForUser");
    }

    @Override
    public void clearHeavyWeightProcessIfEquals(com.android.server.wm.WindowProcessController arg0) {
        throw unsupported("clearHeavyWeightProcessIfEquals");
    }

    @Override
    public void clearLockedTasks(java.lang.String arg0) {
        throw unsupported("clearLockedTasks");
    }

    @Override
    public void clearPendingResultForActivity(android.os.IBinder arg0, java.lang.ref.WeakReference arg1) {
        throw unsupported("clearPendingResultForActivity");
    }

    @Override
    public void closeSystemDialogs(java.lang.String arg0) {
        throw unsupported("closeSystemDialogs");
    }

    @Override
    public com.android.server.wm.ActivityTaskManagerInternal.PackageConfigurationUpdater createPackageConfigurationUpdater() {
        throw unsupported("createPackageConfigurationUpdater");
    }

    @Override
    public com.android.server.wm.ActivityTaskManagerInternal.PackageConfigurationUpdater createPackageConfigurationUpdater(java.lang.String arg0, int arg1) {
        throw unsupported("createPackageConfigurationUpdater");
    }

    @Override
    public void dump(java.lang.String arg0, java.io.FileDescriptor arg1, java.io.PrintWriter arg2, java.lang.String[] arg3, int arg4, boolean arg5, boolean arg6, java.lang.String arg7, int arg8) {
        throw unsupported("dump");
    }

    @Override
    public boolean dumpActivity(java.io.FileDescriptor arg0, java.io.PrintWriter arg1, java.lang.String arg2, java.lang.String[] arg3, int arg4, boolean arg5, boolean arg6, boolean arg7, int arg8, int arg9) {
        throw unsupported("dumpActivity");
    }

    @Override
    public void dumpForOom(java.io.PrintWriter arg0) {
        throw unsupported("dumpForOom");
    }

    @Override
    public boolean dumpForProcesses(java.io.FileDescriptor arg0, java.io.PrintWriter arg1, boolean arg2, java.lang.String arg3, int arg4, boolean arg5, boolean arg6, int arg7) {
        throw unsupported("dumpForProcesses");
    }

    @Override
    public void enableScreenAfterBoot(boolean arg0) {
        throw unsupported("enableScreenAfterBoot");
    }

    @Override
    public void finishHeavyWeightApp() {
        throw unsupported("finishHeavyWeightApp");
    }

    @Override
    public int finishTopCrashedActivities(com.android.server.wm.WindowProcessController arg0, java.lang.String arg1) {
        throw unsupported("finishTopCrashedActivities");
    }

    @Override
    public void flushRecentTasks() {
        throw unsupported("flushRecentTasks");
    }

    @Override
    public android.content.ComponentName getActivityName(android.os.IBinder arg0) {
        throw unsupported("getActivityName");
    }

    @Override
    public java.util.List getAppTasks(java.lang.String arg0, int arg1) {
        throw unsupported("getAppTasks");
    }

    @Override
    public com.android.server.wm.ActivityTaskManagerInternal.PackageConfig getApplicationConfig(java.lang.String arg0, int arg1) {
        throw unsupported("getApplicationConfig");
    }

    @Override
    public com.android.server.wm.ActivityTaskManagerInternal.ActivityTokens getAttachedNonFinishingActivityForTask(int arg0, android.os.IBinder arg1) {
        throw unsupported("getAttachedNonFinishingActivityForTask");
    }

    @Override
    public int getDisplayId(android.os.IBinder arg0) {
        throw unsupported("getDisplayId");
    }

    @Override
    public android.content.ComponentName getHomeActivityForUser(int arg0) {
        throw unsupported("getHomeActivityForUser");
    }

    @Override
    public android.content.Intent getHomeIntent() {
        throw unsupported("getHomeIntent");
    }

    @Override
    public android.content.IIntentSender getIntentSender(int arg0, java.lang.String arg1, java.lang.String arg2, int arg3, int arg4, android.os.IBinder arg5, java.lang.String arg6, int arg7, android.content.Intent[] arg8, java.lang.String[] arg9, int arg10, android.os.Bundle arg11) {
        throw unsupported("getIntentSender");
    }

    @Override
    public com.android.server.wm.ActivityMetricsLaunchObserverRegistry getLaunchObserverRegistry() {
        throw unsupported("getLaunchObserverRegistry");
    }

    @Override
    public android.app.ActivityManager.RecentTaskInfo getMostRecentTaskFromBackground() {
        throw unsupported("getMostRecentTaskFromBackground");
    }

    @Override
    public com.android.server.wm.ActivityServiceConnectionsHolder getServiceConnectionsHolder(android.os.IBinder arg0) {
        throw unsupported("getServiceConnectionsHolder");
    }

    @Override
    public android.window.TaskSnapshot getTaskSnapshotBlocking(int arg0, boolean arg1, int arg2) {
        throw unsupported("getTaskSnapshotBlocking");
    }

    @Override
    public int getTaskToShowPermissionDialogOn(java.lang.String arg0, int arg1) {
        throw unsupported("getTaskToShowPermissionDialogOn");
    }

    @Override
    public com.android.server.wm.WindowProcessController getTopApp() {
        throw unsupported("getTopApp");
    }

    @Override
    public int getTopProcessState() {
        throw unsupported("getTopProcessState");
    }

    @Override
    public java.util.List getTopVisibleActivities() {
        throw unsupported("getTopVisibleActivities");
    }

    @Override
    public android.os.IBinder getUriPermissionOwnerForActivity(android.os.IBinder arg0) {
        throw unsupported("getUriPermissionOwnerForActivity");
    }

    @Override
    public boolean handleAppCrashInActivityController(java.lang.String arg0, int arg1, java.lang.String arg2, java.lang.String arg3, long arg4, java.lang.String arg5, java.lang.Runnable arg6) {
        throw unsupported("handleAppCrashInActivityController");
    }

    @Override
    public void handleAppDied(com.android.server.wm.WindowProcessController arg0, boolean arg1, java.lang.Runnable arg2) {
        throw unsupported("handleAppDied");
    }

    @Override
    public boolean hasResumedActivity(int arg0) {
        throw unsupported("hasResumedActivity");
    }

    @Override
    public boolean hasSystemAlertWindowPermission(int arg0, int arg1, java.lang.String arg2) {
        throw unsupported("hasSystemAlertWindowPermission");
    }

    @Override
    public boolean isAssistDataAllowed() {
        throw unsupported("isAssistDataAllowed");
    }

    @Override
    public boolean isBaseOfLockedTask(java.lang.String arg0) {
        throw unsupported("isBaseOfLockedTask");
    }

    /**
     * RecentTasks.isCallerRecents: the caller shares the app id of the
     * package that owns config_recentsComponentName for the current user.
     */
    @Override
    public boolean isCallerRecents(int callingUid) {
        ComponentName recents = ComponentName.unflattenFromString(Resources.getSystem()
                .getString(com.android.internal.R.string.config_recentsComponentName));
        if (recents == null) return false;
        PackageManagerInternal packages = LocalServices.getService(PackageManagerInternal.class);
        int recentsUid = packages == null ? Process.INVALID_UID
                : packages.getPackageUid(recents.getPackageName(), 0, UserHandle.USER_SYSTEM);
        return recentsUid != Process.INVALID_UID && UserHandle.isSameApp(callingUid, recentsUid);
    }

    @Override
    public boolean isGetTasksAllowed(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("isGetTasksAllowed");
    }

    @Override
    public boolean isNoDisplay(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("isNoDisplay");
    }

    @Override
    public boolean isShuttingDown() {
        throw unsupported("isShuttingDown");
    }

    @Override
    public boolean isSleeping() {
        throw unsupported("isSleeping");
    }

    @Override
    public boolean isUidForeground(int arg0) {
        throw unsupported("isUidForeground");
    }

    @Override
    public void loadRecentTasksForUser(int arg0) {
        throw unsupported("loadRecentTasksForUser");
    }

    @Override
    public void notifyActiveDreamChanged(android.content.ComponentName arg0) {
        throw unsupported("notifyActiveDreamChanged");
    }

    @Override
    public void notifyActiveVoiceInteractionServiceChanged(android.content.ComponentName arg0) {
        throw unsupported("notifyActiveVoiceInteractionServiceChanged");
    }

    @Override
    public void notifyLockedProfile(int arg0) {
        throw unsupported("notifyLockedProfile");
    }

    @Override
    public void onCleanUpApplicationRecord(com.android.server.wm.WindowProcessController arg0) {
        throw unsupported("onCleanUpApplicationRecord");
    }

    @Override
    public boolean onForceStopPackage(java.lang.String arg0, boolean arg1, boolean arg2, int arg3) {
        throw unsupported("onForceStopPackage");
    }

    @Override
    public void onHandleAppCrash(com.android.server.wm.WindowProcessController arg0) {
        throw unsupported("onHandleAppCrash");
    }

    @Override
    public void onLocalVoiceInteractionStarted(android.os.IBinder arg0, android.service.voice.IVoiceInteractionSession arg1, com.android.internal.app.IVoiceInteractor arg2) {
        throw unsupported("onLocalVoiceInteractionStarted");
    }

    @Override
    public void onPackageAdded(java.lang.String arg0, boolean arg1) {
        throw unsupported("onPackageAdded");
    }

    @Override
    public void onPackageDataCleared(java.lang.String arg0, int arg1) {
        throw unsupported("onPackageDataCleared");
    }

    @Override
    public void onPackageReplaced(android.content.pm.ApplicationInfo arg0) {
        throw unsupported("onPackageReplaced");
    }

    @Override
    public void onPackageUninstalled(java.lang.String arg0, int arg1) {
        throw unsupported("onPackageUninstalled");
    }

    @Override
    public void onPackagesSuspendedChanged(java.lang.String[] arg0, boolean arg1, int arg2) {
        throw unsupported("onPackagesSuspendedChanged");
    }

    @Override
    public void onProcessAdded(com.android.server.wm.WindowProcessController arg0) {
        throw unsupported("onProcessAdded");
    }

    @Override
    public void onProcessMapped(int arg0, com.android.server.wm.WindowProcessController arg1) {
        throw unsupported("onProcessMapped");
    }

    @Override
    public void onProcessRemoved(java.lang.String arg0, int arg1) {
        throw unsupported("onProcessRemoved");
    }

    @Override
    public void onProcessUnMapped(int arg0) {
        throw unsupported("onProcessUnMapped");
    }

    @Override
    public void onUidActive(int arg0, int arg1) {
        throw unsupported("onUidActive");
    }

    @Override
    public void onUidInactive(int arg0) {
        throw unsupported("onUidInactive");
    }

    @Override
    public void onUidProcStateChanged(int arg0, int arg1) {
        throw unsupported("onUidProcStateChanged");
    }

    @Override
    public void onUserStopped(int arg0) {
        throw unsupported("onUserStopped");
    }

    @Override
    public com.android.server.wm.ActivityTaskManagerInternal.PreBindInfo preBindApplication(com.android.server.wm.WindowProcessController arg0, android.content.pm.ApplicationInfo arg1) {
        throw unsupported("preBindApplication");
    }

    @Override
    public void registerActivityStartInterceptor(int arg0, com.android.server.wm.ActivityInterceptorCallback arg1) {
        throw unsupported("registerActivityStartInterceptor");
    }

    @Override
    public void registerCompatScaleProvider(int arg0, com.android.server.wm.CompatScaleProvider arg1) {
        throw unsupported("registerCompatScaleProvider");
    }

    @Override
    public void registerScreenObserver(com.android.server.wm.ActivityTaskManagerInternal.ScreenObserver arg0) {
        throw unsupported("registerScreenObserver");
    }

    @Override
    public void removeRecentTasksByPackageName(java.lang.String arg0, int arg1) {
        throw unsupported("removeRecentTasksByPackageName");
    }

    @Override
    public void removeUser(int arg0) {
        throw unsupported("removeUser");
    }

    @Override
    public boolean requestBackGesture() {
        throw unsupported("requestBackGesture");
    }

    @Override
    public void restartTaskActivityProcessIfVisible(int arg0, java.lang.String arg1) {
        throw unsupported("restartTaskActivityProcessIfVisible");
    }

    @Override
    public void resumeTopActivities(boolean arg0) {
        throw unsupported("resumeTopActivities");
    }

    @Override
    public void scheduleDestroyAllActivities(java.lang.String arg0) {
        throw unsupported("scheduleDestroyAllActivities");
    }

    @Override
    public void sendActivityResult(int arg0, android.os.IBinder arg1, java.lang.String arg2, int arg3, int arg4, android.content.Intent arg5) {
        throw unsupported("sendActivityResult");
    }

    @Override
    public void setAccessibilityServiceUids(android.util.IntArray arg0) {
        throw unsupported("setAccessibilityServiceUids");
    }

    @Override
    public void setAllowAppSwitches(java.lang.String arg0, int arg1, int arg2) {
        throw unsupported("setAllowAppSwitches");
    }

    @Override
    public void setBackgroundActivityStartCallback(com.android.server.wm.BackgroundActivityStartCallback arg0) {
        throw unsupported("setBackgroundActivityStartCallback");
    }

    @Override
    public void setCompanionAppUids(int arg0, java.util.Set arg1) {
        throw unsupported("setCompanionAppUids");
    }

    @Override
    public void setDeviceOwnerUid(int arg0) {
        throw unsupported("setDeviceOwnerUid");
    }

    @Override
    public void setProfileApp(java.lang.String arg0) {
        throw unsupported("setProfileApp");
    }

    @Override
    public void setProfileOwnerUids(java.util.Set arg0) {
        throw unsupported("setProfileOwnerUids");
    }

    @Override
    public void setProfileProc(com.android.server.wm.WindowProcessController arg0) {
        throw unsupported("setProfileProc");
    }

    @Override
    public void setProfilerInfo(android.app.ProfilerInfo arg0) {
        throw unsupported("setProfilerInfo");
    }

    @Override
    public void setVr2dDisplayId(int arg0) {
        throw unsupported("setVr2dDisplayId");
    }

    @Override
    public boolean showStrictModeViolationDialog() {
        throw unsupported("showStrictModeViolationDialog");
    }

    @Override
    public void showSystemReadyErrorDialogsIfNeeded() {
        throw unsupported("showSystemReadyErrorDialogsIfNeeded");
    }

    @Override
    public boolean shuttingDown(boolean arg0, int arg1) {
        throw unsupported("shuttingDown");
    }

    @Override
    public int startActivitiesAsPackage(java.lang.String arg0, java.lang.String arg1, int arg2, android.content.Intent[] arg3, android.os.Bundle arg4) {
        throw unsupported("startActivitiesAsPackage");
    }

    @Override
    public int startActivitiesInPackage(int arg0, int arg1, int arg2, java.lang.String arg3, java.lang.String arg4, android.content.Intent[] arg5, java.lang.String[] arg6, android.os.IBinder arg7, com.android.server.wm.SafeActivityOptions arg8, int arg9, boolean arg10, com.android.server.am.PendingIntentRecord arg11, boolean arg12) {
        throw unsupported("startActivitiesInPackage");
    }

    @Override
    public int startActivityAsUser(android.app.IApplicationThread arg0, java.lang.String arg1, java.lang.String arg2, android.content.Intent arg3, android.os.IBinder arg4, int arg5, android.os.Bundle arg6, int arg7) {
        throw unsupported("startActivityAsUser");
    }

    @Override
    public int startActivityInPackage(int arg0, int arg1, int arg2, java.lang.String arg3, java.lang.String arg4, android.content.Intent arg5, java.lang.String arg6, android.os.IBinder arg7, java.lang.String arg8, int arg9, int arg10, com.android.server.wm.SafeActivityOptions arg11, int arg12, com.android.server.wm.Task arg13, java.lang.String arg14, boolean arg15, com.android.server.am.PendingIntentRecord arg16, boolean arg17) {
        throw unsupported("startActivityInPackage");
    }

    @Override
    public int startActivityWithScreenshot(android.content.Intent arg0, java.lang.String arg1, int arg2, int arg3, android.os.IBinder arg4, android.os.Bundle arg5, int arg6) {
        throw unsupported("startActivityWithScreenshot");
    }

    @Override
    public void startConfirmDeviceCredentialIntent(android.content.Intent arg0, android.os.Bundle arg1) {
        throw unsupported("startConfirmDeviceCredentialIntent");
    }

    @Override
    public android.app.IAppTask startDreamActivity(android.content.Intent arg0, int arg1, int arg2) {
        throw unsupported("startDreamActivity");
    }

    @Override
    public boolean startHomeActivity(int arg0, java.lang.String arg1) {
        throw unsupported("startHomeActivity");
    }

    @Override
    public boolean startHomeOnAllDisplays(int arg0, java.lang.String arg1) {
        throw unsupported("startHomeOnAllDisplays");
    }

    @Override
    public boolean startHomeOnDisplay(int arg0, java.lang.String arg1, int arg2, boolean arg3, boolean arg4) {
        throw unsupported("startHomeOnDisplay");
    }

    @Override
    public boolean switchUser(int arg0, com.android.server.am.UserState arg1) {
        throw unsupported("switchUser");
    }

    @Override
    public void unregisterActivityStartInterceptor(int arg0) {
        throw unsupported("unregisterActivityStartInterceptor");
    }

    @Override
    public void updateTopComponentForFactoryTest() {
        throw unsupported("updateTopComponentForFactoryTest");
    }

    @Override
    public void updateUserConfiguration() {
        throw unsupported("updateUserConfiguration");
    }

    @Override
    public boolean useTopSchedGroupForTopProcess() {
        throw unsupported("useTopSchedGroupForTopProcess");
    }

    @Override
    public void writeActivitiesToProto(android.util.proto.ProtoOutputStream arg0) {
        throw unsupported("writeActivitiesToProto");
    }

    @Override
    public void writeProcessesToProto(android.util.proto.ProtoOutputStream arg0, java.lang.String arg1, int arg2, boolean arg3) {
        throw unsupported("writeProcessesToProto");
    }
}
