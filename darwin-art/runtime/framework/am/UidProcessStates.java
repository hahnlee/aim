package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.net.NetworkPolicyManager;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Process;
import android.util.SparseIntArray;
import com.android.server.appop.AppOpsService;
import dev.darwinart.runtime.wm.ActivityClientControllerEndpoint;

/**
 * The uid process state and capabilities ActivityManagerService's OomAdjuster
 * computes from each process's Activities, reported to AppOpsService through
 * updateUidProcState when they change (UidObserver order: state, then
 * capability, together).
 *
 * <p>Processes without Activities are service processes: this runtime keeps
 * an app process alive only for its Activities, services or receivers.</p>
 */
public final class UidProcessStates {
    private static volatile UidProcessStates instance;

    private final ApplicationProcessRegistry processes;
    private final Handler handler;
    private final Runnable refresh = this::refreshAll;
    // Last state/capability reported per uid.
    private final SparseIntArray reportedStates = new SparseIntArray();
    private final SparseIntArray reportedCapabilities = new SparseIntArray();
    private AppOpsService appOps;
    private final UidObserverController observers = new UidObserverController();

    UidProcessStates(ApplicationProcessRegistry processes) {
        if (processes == null) throw new NullPointerException("processes");
        this.processes = processes;
        HandlerThread thread = new HandlerThread("DarwinUidStates");
        thread.start();
        handler = new Handler(thread.getLooper());
        instance = this;
        processes.setChangeListener(UidProcessStates::changed);
    }

    /** The registered IUidObservers these states are reported to. */
    UidObserverController observers() {
        return observers;
    }

    /** ActivityManagerService's AppOpsService, once SystemServer created it. */
    void attach(AppOpsService service) {
        handler.post(() -> {
            appOps = service;
            refreshAll();
        });
    }

    /**
     * A process attached or died, or an Activity changed lifecycle state.
     * Coalesced and recomputed on this owner's thread, outside every caller's
     * lock.
     */
    public static void changed() {
        UidProcessStates current = instance;
        if (current == null) return;
        current.handler.removeCallbacks(current.refresh);
        current.handler.post(current.refresh);
    }

    /** OomAdjuster's process state of one attached application process. */
    public static int processState(IBinder applicationThread) {
        int activities = ActivityClientControllerEndpoint.processState(applicationThread);
        return activities == ActivityManager.PROCESS_STATE_NONEXISTENT
                ? ActivityManager.PROCESS_STATE_SERVICE : activities;
    }

    /** OomAdjuster.getDefaultCapability for a process in {@code state}. */
    static int capability(int state) {
        int base = state <= ActivityManager.PROCESS_STATE_TOP
                ? ActivityManager.PROCESS_CAPABILITY_ALL
                : ActivityManager.PROCESS_CAPABILITY_NONE;
        return base | NetworkPolicyManager.getDefaultProcessNetworkCapabilities(state);
    }

    private void refreshAll() {
        if (appOps == null) return;
        SparseIntArray states = new SparseIntArray();
        SparseIntArray capabilities = new SparseIntArray();
        for (ApplicationProcessRegistry.AttachedApplication process : processes.attached()) {
            // The system process is persistent; AppOps never gates it.
            if (process.uid == Process.SYSTEM_UID) continue;
            int state = processState(process.thread);
            int current = states.get(process.uid, ActivityManager.PROCESS_STATE_NONEXISTENT);
            states.put(process.uid, Math.min(current, state));
            capabilities.put(process.uid, capabilities.get(process.uid) | capability(state));
        }
        for (int i = 0; i < states.size(); i++) {
            report(states.keyAt(i), states.valueAt(i), capabilities.get(states.keyAt(i)));
        }
        // A uid whose last process died.
        for (int i = reportedStates.size() - 1; i >= 0; i--) {
            int uid = reportedStates.keyAt(i);
            if (states.indexOfKey(uid) < 0) {
                report(uid, ActivityManager.PROCESS_STATE_NONEXISTENT,
                        ActivityManager.PROCESS_CAPABILITY_NONE);
            }
        }
    }

    private void report(int uid, int state, int capability) {
        int previous = reportedStates.get(uid, ActivityManager.PROCESS_STATE_NONEXISTENT);
        if (previous == state && reportedCapabilities.get(uid) == capability
                && (reportedStates.indexOfKey(uid) >= 0
                        || state == ActivityManager.PROCESS_STATE_NONEXISTENT)) {
            return;
        }
        appOps.updateUidProcState(uid, state, capability);
        observers.dispatch(uid, reportedStates.indexOfKey(uid) >= 0
                ? previous : ActivityManager.PROCESS_STATE_NONEXISTENT, state,
                reportedCapabilities.get(uid), capability);
        if (state == ActivityManager.PROCESS_STATE_NONEXISTENT) {
            reportedStates.delete(uid);
            reportedCapabilities.delete(uid);
        } else {
            reportedStates.put(uid, state);
            reportedCapabilities.put(uid, capability);
        }
    }
}
