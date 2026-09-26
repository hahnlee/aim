package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import java.util.ArrayList;
import java.util.List;

/**
 * IActivityManager.getRunningAppProcesses policy.
 *
 * <p>An app without REAL_GET_TASKS sees only processes of its own uid
 * (ActivityManagerService.getRunningAppProcesses). Importance is the
 * process state's (UidProcessStates.processState).</p>
 */
final class RunningAppProcesses {
    private RunningAppProcesses() {}

    static List<ActivityManager.RunningAppProcessInfo> forCaller(
            ApplicationProcessRegistry processes, int callingUid) {
        ArrayList<ActivityManager.RunningAppProcessInfo> result = new ArrayList<>();
        for (ApplicationProcessRegistry.AttachedApplication process
                : processes.attachedForUid(callingUid)) {
            ActivityManager.RunningAppProcessInfo info = new ActivityManager.RunningAppProcessInfo(
                    process.processName, process.pid, new String[] {process.packageName});
            info.uid = process.uid;
            info.importance = ActivityManager.RunningAppProcessInfo.procStateToImportance(
                    UidProcessStates.processState(process.thread));
            result.add(info);
        }
        return result;
    }
}
