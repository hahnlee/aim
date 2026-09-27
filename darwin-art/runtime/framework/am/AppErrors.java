package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.app.ApplicationErrorReport;
import android.os.Process;
import android.util.Log;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;

/**
 * ActivityManagerService's AppErrors: a process that reported an uncaught
 * exception (KillApplicationHandler, just before it kills itself) is held in
 * the CRASHED error state until it is gone, and one whose input dispatching
 * timed out (ProcessErrorStateRecord.appNotResponding) in NOT_RESPONDING until
 * it is gone or crashes. getProcessesInErrorState reports them to their own
 * uid, or to the system. There is no crash/ANR dialog yet.
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
     * appNotResponding for {@code pid}: records NOT_RESPONDING, logs the ANR
     * as ActivityManager does, and asks ART for the process's stack dump
     * (SIGQUIT, which its Signal Catcher writes to the log).
     */
    void notResponding(int pid, int uid, String processName, String annotation) {
        ActivityManager.ProcessErrorStateInfo info = new ActivityManager.ProcessErrorStateInfo();
        info.condition = ActivityManager.ProcessErrorStateInfo.NOT_RESPONDING;
        info.processName = processName;
        info.pid = pid;
        info.uid = uid;
        info.tag = null;
        info.shortMsg = "ANR";
        info.longMsg = "ANR in " + processName + "\nReason: " + annotation;
        synchronized (crashed) {
            // A crash already reported for the process takes precedence.
            ActivityManager.ProcessErrorStateInfo existing = crashed.get(pid);
            if (existing != null
                    && existing.condition == ActivityManager.ProcessErrorStateInfo.CRASHED) {
                return;
            }
            crashed.put(pid, info);
        }
        Log.e(TAG, "ANR in " + processName + " (pid " + pid + ")\nReason: " + annotation);
        Process.sendSignal(pid, Process.SIGNAL_QUIT);
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
