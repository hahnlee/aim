package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.app.ApplicationErrorReport;
import android.os.Process;
import android.util.Log;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;

/**
 * ActivityManagerService's AppErrors for crashes: a process that reported an
 * uncaught exception (KillApplicationHandler, just before it kills itself)
 * is held in the CRASHED error state until it is gone, and
 * getProcessesInErrorState reports it to its own uid, or to the system.
 * There is no ANR detection or crash dialog yet.
 */
final class AppErrors {
    private static final String TAG = "DarwinAppErrors";

    private final ApplicationProcessRegistry processes;
    private final HashMap<Integer, ActivityManager.ProcessErrorStateInfo> crashed =
            new HashMap<>();

    AppErrors(ApplicationProcessRegistry processes) {
        this.processes = processes;
    }

    /** handleApplicationCrash from the crashing process {@code pid}. */
    void crashApplication(int pid, int uid, String packageName,
            ApplicationErrorReport.ParcelableCrashInfo crash) {
        ActivityManager.ProcessErrorStateInfo info = new ActivityManager.ProcessErrorStateInfo();
        info.condition = ActivityManager.ProcessErrorStateInfo.CRASHED;
        info.processName = packageName;
        info.pid = pid;
        info.uid = uid;
        info.tag = null;
        info.shortMsg = crash == null ? null : crash.exceptionClassName;
        info.longMsg = crash == null ? null
                : crash.exceptionClassName + ": " + crash.exceptionMessage;
        info.stackTrace = crash == null ? null : crash.stackTrace;
        synchronized (crashed) {
            crashed.put(pid, info);
        }
        Log.e(TAG, "crash pid=" + pid + " package=" + packageName + " "
                + (info.longMsg == null ? "(no report)" : info.longMsg)
                + (crash == null ? "" : " at " + crash.throwClassName + "."
                        + crash.throwMethodName + "(" + crash.throwFileName + ":"
                        + crash.throwLineNumber + ")"));
    }

    /**
     * getProcessesInErrorState: the caller's own crashed processes (all of
     * them for the system), or null when there are none.
     */
    List<ActivityManager.ProcessErrorStateInfo> errorStates(int callingUid) {
        ArrayList<ActivityManager.ProcessErrorStateInfo> result = new ArrayList<>();
        synchronized (crashed) {
            crashed.keySet().removeIf(pid -> !attached(pid));
            for (ActivityManager.ProcessErrorStateInfo info : crashed.values()) {
                if (callingUid == Process.SYSTEM_UID || info.uid == callingUid) result.add(info);
            }
        }
        return result.isEmpty() ? null : result;
    }

    private boolean attached(int pid) {
        for (ApplicationProcessRegistry.AttachedApplication app : processes.attached()) {
            if (app.pid == pid) return true;
        }
        return false;
    }
}
