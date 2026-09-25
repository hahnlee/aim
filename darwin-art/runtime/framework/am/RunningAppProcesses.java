package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import dev.darwinart.runtime.wm.ActivityClientControllerEndpoint;
import java.util.ArrayList;
import java.util.List;

/**
 * IActivityManager.getRunningAppProcesses policy.
 *
 * <p>An app without REAL_GET_TASKS sees only processes of its own uid
 * (ActivityManagerService.getRunningAppProcesses). Importance follows the
 * process's Activity state: a resumed Activity is foreground, a paused but
 * live Activity stays visible in its desktop window, and a process without
 * Activities is hosting services.</p>
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
            info.importance = importance(
                    ActivityClientControllerEndpoint.activityPresence(process.thread));
            result.add(info);
        }
        return result;
    }

    /** ActivityManager.PROCESS_STATE_* reported to the process with its callbacks. */
    static int processState(int activityPresence) {
        switch (activityPresence) {
            case 2: return 2; // PROCESS_STATE_TOP
            case 1: return 6; // PROCESS_STATE_IMPORTANT_FOREGROUND
            default: return 10; // PROCESS_STATE_SERVICE
        }
    }

    static int importance(int activityPresence) {
        switch (activityPresence) {
            case 2: return ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND;
            case 1: return ActivityManager.RunningAppProcessInfo.IMPORTANCE_VISIBLE;
            default: return ActivityManager.RunningAppProcessInfo.IMPORTANCE_SERVICE;
        }
    }
}
