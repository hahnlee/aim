package dev.darwinart.runtime.wm;

import android.app.ActivityManager;
import android.app.servertransaction.ActivityLifecycleItem;
import android.content.pm.ActivityInfo;
import android.content.res.Configuration;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcel;
import android.os.PersistableBundle;
import android.os.RemoteException;
import android.text.TextUtils;
import android.util.Log;
import android.view.IWindow;
import dev.darwinart.runtime.am.UidProcessStates;
import java.lang.reflect.Field;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;

/** System-owned activity-token state exposed through IActivityClientController. */
public final class ActivityClientControllerEndpoint extends Binder {
    private static final String TAG = "DarwinActivityClient";
    // ActivityTaskSupervisor.IDLE_TIMEOUT: stop the Activities a new top
    // Activity hides even if it never reports idle.
    private static final long IDLE_TIMEOUT_MILLIS = 10_000;

    private enum State { RESUMED, PAUSED, STOPPING, STOPPED }

    private static final class ActivityRecord {
        final IBinder applicationThread;
        final ActivityInfo info;
        // ActivityRecord.mOccludesParent from the Activity's window style.
        final boolean occludesParent;
        State state = State.RESUMED;
        // The Activity the launcher started this task with (the process's
        // initial launch): Back there moves the task to the back.
        boolean launcherRoot;
        int requestedOrientation;
        // Merged configuration most recently sent to the Activity by a launch,
        // configuration change or relaunch transaction.
        Configuration reported;

        ActivityRecord(IBinder applicationThread, ActivityInfo info, boolean occludesParent,
                Configuration reported) {
            this.applicationThread = applicationThread;
            this.info = info;
            this.occludesParent = occludesParent;
            this.requestedOrientation = info == null
                    ? ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED : info.screenOrientation;
            this.reported = reported == null ? null : new Configuration(reported);
        }
    }

    /** Immutable view of one live Activity for task configuration dispatch. */
    static final class ActivitySnapshot {
        final IBinder token;
        // The ActivityLifecycleItem state a relaunch returns the Activity to.
        final int lifecycleState;
        final int configChanges;
        final int targetSdkVersion;
        final Configuration reported;

        ActivitySnapshot(IBinder token, ActivityRecord record) {
            this.token = token;
            lifecycleState = record.state == State.RESUMED ? ActivityLifecycleItem.ON_RESUME
                    : record.state == State.PAUSED ? ActivityLifecycleItem.ON_PAUSE
                    : ActivityLifecycleItem.ON_STOP;
            configChanges = record.info == null ? 0 : record.info.configChanges;
            targetSdkVersion = record.info == null || record.info.applicationInfo == null
                    ? 10_000 : record.info.applicationInfo.targetSdkVersion;
            reported = record.reported == null ? null : new Configuration(record.reported);
        }
    }

    private static final Object activityLock = new Object();
    private static final HashMap<IBinder, ActivityRecord> activityRecords = new HashMap<>();
    private static final HashMap<IBinder, ArrayDeque<IBinder>> activityStacks = new HashMap<>();
    private final int getDisplayIdCode = transaction("getDisplayId");
    private final int activityIdleCode = transaction("activityIdle");
    private final int activityStoppedCode = transaction("activityStopped");
    private final int willActivityBeVisibleCode = transaction("willActivityBeVisible");
    private final int activityResumedCode = transaction("activityResumed");
    private final int activityPausedCode = transaction("activityPaused");
    private final int activityDestroyedCode = transaction("activityDestroyed");
    private final int activityRelaunchedCode = transaction("activityRelaunched");
    private final int finishActivityCode = transaction("finishActivity");
    private final int onBackPressedCode = transaction("onBackPressed");
    private final int setRequestedOrientationCode = transaction("setRequestedOrientation");
    private final int getRequestedOrientationCode = transaction("getRequestedOrientation");

    public ActivityClientControllerEndpoint() {
        attachInterface(null, "android.app.IActivityClientController");
    }

    /** Initial launcher registration from the AMS attach launch transaction. */
    public static void registerActivityToken(IBinder applicationThread, IBinder token,
            ActivityInfo info, Configuration reported) {
        if (applicationThread == null || token == null) {
            throw new IllegalArgumentException("Activity application thread/token is null");
        }
        boolean occludesParent = ActivityWindowStyle.occludesParent(info);
        synchronized (activityLock) {
            commitLaunchLocked(applicationThread, token, info, occludesParent, reported);
            ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
            ActivityRecord record = activityRecords.get(token);
            if (record != null && stack != null && stack.size() == 1) record.launcherRoot = true;
        }
        UidProcessStates.changed();
        scheduleIdleTimeout(token);
    }

    IBinder topActivityToken(IBinder applicationThread) {
        synchronized (activityLock) {
            return topActivityTokenLocked(applicationThread);
        }
    }

    void commitLaunch(IBinder applicationThread, IBinder token, ActivityInfo info,
            Configuration reported) {
        boolean occludesParent = ActivityWindowStyle.occludesParent(info);
        synchronized (activityLock) {
            IBinder previous = topActivityTokenLocked(applicationThread);
            if (previous != null) activityRecords.get(previous).state = State.PAUSED;
            commitLaunchLocked(applicationThread, token, info, occludesParent, reported);
        }
        UidProcessStates.changed();
        scheduleIdleTimeout(token);
    }

    /** Requested orientation of the process's top Activity, or unspecified. */
    static int topRequestedOrientation(IBinder applicationThread) {
        synchronized (activityLock) {
            IBinder top = topActivityTokenLocked(applicationThread);
            return top == null ? ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
                    : activityRecords.get(top).requestedOrientation;
        }
    }

    /** Live Activities of one process, bottom to top. */
    static List<ActivitySnapshot> activities(IBinder applicationThread) {
        ArrayList<ActivitySnapshot> result = new ArrayList<>();
        synchronized (activityLock) {
            ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
            if (stack == null) return result;
            for (IBinder token : stack) {
                result.add(new ActivitySnapshot(token, activityRecords.get(token)));
            }
        }
        return result;
    }

    /**
     * OomAdjuster's Activity contribution to one process's state: TOP for a
     * resumed or paused (visible) Activity, LAST_ACTIVITY while one stops,
     * CACHED_ACTIVITY for stopped ones, NONEXISTENT without Activities.
     */
    public static int processState(IBinder applicationThread) {
        synchronized (activityLock) {
            ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
            int result = ActivityManager.PROCESS_STATE_NONEXISTENT;
            if (stack == null) return result;
            for (IBinder token : stack) {
                switch (activityRecords.get(token).state) {
                    case RESUMED:
                    case PAUSED:
                        return ActivityManager.PROCESS_STATE_TOP;
                    case STOPPING:
                        result = Math.min(result, ActivityManager.PROCESS_STATE_LAST_ACTIVITY);
                        break;
                    case STOPPED:
                        result = Math.min(result, ActivityManager.PROCESS_STATE_CACHED_ACTIVITY);
                        break;
                }
            }
            return result;
        }
    }

    /** Records the configuration carried by a scheduled task transaction. */
    static void reported(IBinder token, Configuration configuration) {
        synchronized (activityLock) {
            ActivityRecord record = activityRecords.get(token);
            if (record != null) record.reported = new Configuration(configuration);
        }
    }

    /** Process retirement drops every record owned by that application thread. */
    static void removeProcess(IBinder applicationThread) {
        synchronized (activityLock) {
            ArrayDeque<IBinder> stack = activityStacks.remove(applicationThread);
            if (stack == null) return;
            for (IBinder token : stack) activityRecords.remove(token);
        }
        UidProcessStates.changed();
    }

    private static void commitLaunchLocked(IBinder applicationThread, IBinder token,
            ActivityInfo info, boolean occludesParent, Configuration reported) {
        if (activityRecords.containsKey(token)) return;
        activityRecords.put(token,
                new ActivityRecord(applicationThread, info, occludesParent, reported));
        activityStacks.computeIfAbsent(applicationThread, unused -> new ArrayDeque<>())
                .addLast(token);
    }

    private static IBinder topActivityTokenLocked(IBinder applicationThread) {
        ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
        return stack == null ? null : stack.peekLast();
    }

    /**
     * ActivityTaskSupervisor.activityIdleInternal: Activities hidden behind an
     * occluding Activity of their task are stopped once the new top Activity
     * is idle. Their windows lose app visibility first, as WMS commits
     * visibility before the stop transaction.
     */
    private static void stopHiddenActivities(IBinder applicationThread) {
        List<IBinder> stopping;
        synchronized (activityLock) {
            stopping = hiddenActivitiesLocked(applicationThread, State.PAUSED);
            for (IBinder token : stopping) activityRecords.get(token).state = State.STOPPING;
        }
        if (stopping.isEmpty()) return;
        UidProcessStates.changed();
        WindowSurfaceRegistry windows = TaskGeometryController.requireInstance().windows();
        ArrayList<android.app.servertransaction.ClientTransactionItem> items = new ArrayList<>();
        for (IBinder token : stopping) {
            dispatchAppVisibility(windows, token, false);
            items.add(TaskClientTransactions.stop(token));
        }
        Log.i(TAG, "stopping " + stopping.size() + " hidden activities");
        try {
            TaskClientTransactions.schedule(applicationThread, items);
        } catch (RemoteException error) {
            Log.w(TAG, "stop transaction not delivered", error);
        }
    }

    /**
     * Activities of one task in {@code state} that an occluding Activity above
     * them hides (ActivityRecord.shouldBeVisible), bottom to top.
     */
    private static List<IBinder> hiddenActivitiesLocked(IBinder applicationThread,
            State state) {
        ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
        if (stack == null) return Collections.emptyList();
        ArrayList<IBinder> hidden = new ArrayList<>();
        boolean behindOccluding = false;
        for (Iterator<IBinder> top = stack.descendingIterator(); top.hasNext(); ) {
            IBinder token = top.next();
            ActivityRecord record = activityRecords.get(token);
            if (behindOccluding && record.state == state) hidden.add(0, token);
            if (record.occludesParent) behindOccluding = true;
        }
        return hidden;
    }

    /** Whether no occluding Activity is above {@code token} in its task. */
    private static boolean visibleLocked(IBinder token) {
        ActivityRecord record = requireActivityLocked(token);
        ArrayDeque<IBinder> stack = activityStacks.get(record.applicationThread);
        if (stack == null) return false;
        for (Iterator<IBinder> top = stack.descendingIterator(); top.hasNext(); ) {
            IBinder above = top.next();
            if (above.equals(token)) return true;
            if (activityRecords.get(above).occludesParent) return false;
        }
        return false;
    }

    /** WindowToken.sendAppVisibilityToClients for one Activity's windows. */
    private static void dispatchAppVisibility(WindowSurfaceRegistry windows, IBinder token,
            boolean visible) {
        for (IBinder window : windows.activityWindows(token)) {
            try {
                IWindow.Stub.asInterface(window).dispatchAppVisibility(visible);
            } catch (RemoteException error) {
                Log.w(TAG, "app visibility not delivered to a dead window");
            }
        }
    }

    /**
     * ActivityTaskManager moveTaskToBack / moveTaskToFront for the user hiding
     * or reopening the task's root: every visible Activity loses app
     * visibility and pauses and stops; reopening restores visibility and
     * resumes the top Activity (restart from stopped), as Home and returning
     * to the app do on Android.
     */
    static void setTaskHidden(IBinder applicationThread, boolean hidden) {
        ArrayList<IBinder> stopping = new ArrayList<>();
        IBinder top;
        boolean topWasResumed = false;
        boolean resumeTop = false;
        synchronized (activityLock) {
            ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
            if (stack == null || stack.isEmpty()) return;
            top = stack.peekLast();
            if (hidden) {
                for (IBinder token : stack) {
                    ActivityRecord record = activityRecords.get(token);
                    if (record.state == State.RESUMED || record.state == State.PAUSED) {
                        if (token == top) topWasResumed = record.state == State.RESUMED;
                        record.state = State.STOPPING;
                        stopping.add(token);
                    }
                }
            } else {
                ActivityRecord record = activityRecords.get(top);
                if (record.state == State.STOPPING || record.state == State.STOPPED) {
                    record.state = State.RESUMED;
                    resumeTop = true;
                }
            }
        }
        if (stopping.isEmpty() && !resumeTop) return;
        WindowSurfaceRegistry windows = TaskGeometryController.requireInstance().windows();
        ArrayList<android.app.servertransaction.ClientTransactionItem> items = new ArrayList<>();
        for (IBinder token : stopping) {
            dispatchAppVisibility(windows, token, false);
            if (token == top && topWasResumed) {
                items.add(new android.app.servertransaction.PauseActivityItem(token));
            }
            items.add(TaskClientTransactions.stop(token));
        }
        if (resumeTop) {
            dispatchAppVisibility(windows, top, true);
            items.add(new android.app.servertransaction.ResumeActivityItem(
                    top, false /* isForward */, false /* shouldSendCompatFakeFocus */));
        }
        try {
            TaskClientTransactions.schedule(applicationThread, items);
        } catch (RemoteException error) {
            Log.w(TAG, "task visibility transaction not delivered", error);
        }
        UidProcessStates.changed();
        Log.i(TAG, (hidden ? "task to back, stopping " + stopping.size() : "task to front")
                + " activities");
    }

    /**
     * ActivityTaskManager removeTask for the user quitting the app: every
     * Activity is destroyed (the executor pauses and stops it first), then
     * the process ends once they are gone or after a bounded wait (ADR 0011).
     */
    static void removeTask(IBinder applicationThread, int pid) {
        ArrayList<IBinder> tokens = new ArrayList<>();
        synchronized (activityLock) {
            ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
            if (stack != null) {
                for (java.util.Iterator<IBinder> top = stack.descendingIterator(); top.hasNext(); ) {
                    tokens.add(top.next());
                }
            }
        }
        ArrayList<android.app.servertransaction.ClientTransactionItem> items = new ArrayList<>();
        for (IBinder token : tokens) {
            items.add(new android.app.servertransaction.DestroyActivityItem(token, true));
        }
        try {
            TaskClientTransactions.schedule(applicationThread, items);
        } catch (RemoteException error) {
            Log.w(TAG, "task removal transaction not delivered", error);
        }
        Log.i(TAG, "remove task pid=" + pid + " destroying " + tokens.size() + " activities");
        long deadline = android.os.SystemClock.uptimeMillis() + 5_000;
        Runnable[] reap = new Runnable[1];
        reap[0] = () -> {
            boolean gone;
            synchronized (activityLock) {
                ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
                gone = stack == null || stack.isEmpty();
            }
            if (gone || android.os.SystemClock.uptimeMillis() >= deadline) {
                Log.i(TAG, "remove task pid=" + pid + " ending process"
                        + (gone ? "" : " after timeout"));
                android.os.Process.killProcess(pid);
            } else {
                TaskGeometryController.requireInstance().post(reap[0], 100);
            }
        };
        TaskGeometryController.requireInstance().post(reap[0], 100);
    }

    private static void scheduleIdleTimeout(IBinder token) {
        TaskGeometryController.requireInstance().post(() -> {
            IBinder applicationThread;
            synchronized (activityLock) {
                ActivityRecord record = activityRecords.get(token);
                if (record == null || record.state != State.RESUMED) return;
                applicationThread = record.applicationThread;
            }
            stopHiddenActivities(applicationThread);
        }, IDLE_TIMEOUT_MILLIS);
    }

    private static ActivityRecord requireActivityLocked(IBinder token) {
        ActivityRecord record = activityRecords.get(token);
        if (record == null) throw new IllegalArgumentException("Unknown Activity token");
        return record;
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.app.IActivityClientController$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != getDisplayIdCode
                && code != activityIdleCode
                && code != activityStoppedCode
                && code != willActivityBeVisibleCode
                && code != activityResumedCode
                && code != activityPausedCode
                && code != activityDestroyedCode
                && code != activityRelaunchedCode
                && code != finishActivityCode
                && code != onBackPressedCode
                && code != setRequestedOrientationCode
                && code != getRequestedOrientationCode) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface("android.app.IActivityClientController");
        IBinder token = data.readStrongBinder();
        if (code == activityIdleCode) {
            data.readTypedObject(Configuration.CREATOR); // Created configuration.
            data.readBoolean(); // stopProfiling
            data.enforceNoDataAvail();
            IBinder applicationThread;
            synchronized (activityLock) {
                ActivityRecord record = activityRecords.get(token);
                // A finished Activity reports idle after its record retired.
                if (record == null) return true;
                applicationThread = record.applicationThread;
            }
            TaskGeometryController.requireInstance().post(
                    () -> stopHiddenActivities(applicationThread), 0);
            return true;
        }
        if (code == activityStoppedCode) {
            data.readTypedObject(Bundle.CREATOR); // Saved instance state.
            data.readTypedObject(PersistableBundle.CREATOR);
            data.readTypedObject(TextUtils.CHAR_SEQUENCE_CREATOR); // Description.
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                ActivityRecord record = activityRecords.get(token);
                // A stop overtaken by a resume (the top Activity finished)
                // leaves the resumed state in place.
                if (record != null && record.state == State.STOPPING) {
                    record.state = State.STOPPED;
                }
            }
            UidProcessStates.changed();
            return true;
        }
        if (code == willActivityBeVisibleCode) {
            data.enforceNoDataAvail();
            boolean visible;
            synchronized (activityLock) {
                visible = visibleLocked(token);
            }
            reply.writeNoException();
            reply.writeBoolean(visible);
            return true;
        }
        if (code == activityResumedCode) {
            data.readBoolean(); // handleSplashScreenExit
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                requireActivityLocked(token).state = State.RESUMED;
            }
            UidProcessStates.changed();
            return true;
        }
        if (code == activityPausedCode) {
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                requireActivityLocked(token).state = State.PAUSED;
            }
            UidProcessStates.changed();
            reply.writeNoException();
            return true;
        }
        if (code == activityRelaunchedCode) {
            // ActivityRelaunchItem.postExecute: the record kept its token and
            // the relaunch's lifecycle item reports the resulting state.
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                requireActivityLocked(token);
            }
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == setRequestedOrientationCode) {
            int requested = data.readInt();
            data.enforceNoDataAvail();
            IBinder applicationThread;
            boolean top;
            synchronized (activityLock) {
                ActivityRecord record = requireActivityLocked(token);
                if (record.requestedOrientation == requested) {
                    if (reply != null) reply.writeNoException();
                    return true;
                }
                record.requestedOrientation = requested;
                applicationThread = record.applicationThread;
                top = token.equals(topActivityTokenLocked(applicationThread));
            }
            // ActivityTask resolves the task extent outside the record lock;
            // only the top Activity decides the desktop task's orientation.
            if (top) TaskGeometryController.requireInstance()
                    .requestedOrientationChanged(applicationThread, requested);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == getRequestedOrientationCode) {
            data.enforceNoDataAvail();
            int requested;
            synchronized (activityLock) {
                requested = requireActivityLocked(token).requestedOrientation;
            }
            reply.writeNoException();
            reply.writeInt(requested);
            return true;
        }
        if (code == activityDestroyedCode) {
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                ActivityRecord record = activityRecords.remove(token);
                if (record != null) {
                    ArrayDeque<IBinder> stack = activityStacks.get(record.applicationThread);
                    if (stack != null) stack.remove(token);
                }
            }
            UidProcessStates.changed();
            return true;
        }
        if (code == onBackPressedCode) {
            // ActivityClientController.onBackPressed: Back on the task's root
            // started from the launcher moves the task to the back, keeping
            // its state as Home does; anywhere else the Activity finishes
            // itself through the request-finish callback (finishActivity).
            IBinder callback = data.readStrongBinder();
            data.enforceNoDataAvail();
            IBinder moveToBack = null;
            boolean known;
            synchronized (activityLock) {
                ActivityRecord record = activityRecords.get(token);
                known = record != null;
                ArrayDeque<IBinder> stack =
                        record == null ? null : activityStacks.get(record.applicationThread);
                if (record != null && record.launcherRoot && stack != null
                        && token.equals(stack.peekFirst())) {
                    moveToBack = record.applicationThread;
                }
            }
            if (moveToBack != null) {
                TaskGeometryController.requireInstance().requestHostHide(moveToBack);
            } else if (known && callback != null) {
                android.app.IRequestFinishCallback.Stub.asInterface(callback).requestFinish();
            }
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == finishActivityCode) {
            data.readInt(); // Result code.
            data.readTypedObject(android.content.Intent.CREATOR); // Result data.
            data.readInt(); // finishTask mode.
            data.enforceNoDataAvail();
            boolean scheduled;
            IBinder revealedThread = null;
            IBinder revealedHidden = null;
            int revealedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
            synchronized (activityLock) {
                ActivityRecord record = requireActivityLocked(token);
                ArrayDeque<IBinder> stack = activityStacks.get(record.applicationThread);
                if (stack == null || !token.equals(stack.peekLast())) {
                    reply.writeNoException();
                    reply.writeBoolean(false);
                    return true;
                }
                IBinder previous = null;
                if (stack.size() > 1) {
                    token = stack.removeLast();
                    previous = stack.peekLast();
                    stack.addLast(token);
                }
                scheduled = ActivityTaskManagerEndpoint.nativeScheduleFinishActivity(
                        record.applicationThread, token, previous);
                if (scheduled) {
                    stack.removeLast();
                    // Keep the finishing record alive through PauseActivityItem's
                    // synchronous activityPaused report. activityDestroyed owns
                    // final retirement, matching the framework callback order.
                    record.state = State.PAUSED;
                    if (previous != null) {
                        ActivityRecord revealed = activityRecords.get(previous);
                        if (revealed.state == State.STOPPING
                                || revealed.state == State.STOPPED) {
                            revealedHidden = previous;
                        }
                        revealed.state = State.RESUMED;
                        revealedThread = record.applicationThread;
                        revealedOrientation = revealed.requestedOrientation;
                    }
                }
            }
            UidProcessStates.changed();
            // ResumeActivityItem restarts a stopped Activity; its windows
            // regain app visibility for the restart's relayout.
            if (revealedHidden != null) {
                IBinder shown = revealedHidden;
                TaskGeometryController controller = TaskGeometryController.requireInstance();
                controller.post(() -> dispatchAppVisibility(controller.windows(), shown, true), 0);
            }
            // The revealed Activity now owns the task's requested orientation.
            if (revealedThread != null) TaskGeometryController.requireInstance()
                    .requestedOrientationChanged(revealedThread, revealedOrientation);
            reply.writeNoException();
            reply.writeBoolean(scheduled);
            return true;
        }
        data.enforceNoDataAvail();
        synchronized (activityLock) {
            requireActivityLocked(token);
        }
        reply.writeNoException();
        reply.writeInt(0); // The first Android logical display.
        return true;
    }
}
