package dev.aim.server;

import android.app.ActivityManager.RunningTaskInfo;
import android.app.WindowConfiguration;
import android.content.pm.ActivityInfo;
import android.os.Binder;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.util.Slog;
import android.view.SurfaceControl;
import android.view.WindowManager;
import android.window.ITransitionPlayer;
import android.window.TaskAppearedInfo;
import android.window.TaskOrganizer;
import android.window.TransitionInfo;
import android.window.TransitionRequestInfo;
import android.window.WindowContainerTransaction;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The device's window shell in place of WMShell (docs/task-organizer.md):
 * the task organizer and the transition player, which make window mode
 * Android's desktop windowing. The default display stays fullscreen and
 * its tasks are freeform, so a start into a running task keeps its bounds
 * (#613). The player applies each transition at once, as WindowManager
 * does when a player fails; in window mode it makes a new fullscreen task
 * freeform in its transition's request. The organizer tells aim-windows
 * each task's activity type and its top activity's orientation (#593).
 *
 * <p>WindowManager calls it in-process, under its lock: every callback
 * runs on the shell's own thread.
 */
final class WindowShell extends TaskOrganizer {
    private static final String TAG = "AimWindowShell";
    /** Set in the lightweight shell, the image the window shell is for (its rc). */
    static final String PROPERTY = "ro.vendor.aim.lightweight_shell";
    /** Display.DEFAULT_DISPLAY. */
    private static final int DEFAULT_DISPLAY = 0;
    /** Its name in servicemanager, for aim-windows. */
    private static final String SERVICE = "aim.window_shell";

    private final Handler mHandler;
    /** The organized tasks; on the shell's thread only. */
    private final Map<Integer, Task> mTasks = new HashMap<>();
    /** aim-windows in window mode, null otherwise; on the shell's thread only. */
    private IWindowShellListener mWindows;

    private static final class Task {
        final SurfaceControl leash;
        int activityType;
        int orientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;

        Task(SurfaceControl leash) {
            this.leash = leash;
        }
    }

    private WindowShell(Handler handler) {
        super(null, handler::post);
        mHandler = handler;
    }

    /** Registers the organizer and the player, and publishes the shell. */
    static void start() {
        HandlerThread thread = new HandlerThread(TAG);
        thread.start();
        WindowShell shell = new WindowShell(thread.getThreadHandler());
        for (TaskAppearedInfo info : shell.registerOrganizer()) {
            shell.mHandler.post(() -> shell.onTaskAppeared(info.getTaskInfo(), info.getLeash()));
        }
        shell.registerTransitionPlayer(shell.new Player());
        ServiceManager.addService(SERVICE, shell.new Service());
        Slog.i(TAG, "task organizer and transition player registered");
    }

    @Override
    public void onTaskAppeared(RunningTaskInfo info, SurfaceControl leash) {
        Task task = new Task(leash);
        mTasks.put(info.taskId, task);
        update(info, task);
        report(info.taskId, task);
    }

    @Override
    public void onTaskInfoChanged(RunningTaskInfo info) {
        Task task = mTasks.get(info.taskId);
        if (task != null && update(info, task)) {
            report(info.taskId, task);
        }
    }

    @Override
    public void onTaskVanished(RunningTaskInfo info) {
        Task task = mTasks.remove(info.taskId);
        if (task != null) {
            task.leash.release();
        }
    }

    /** Takes what `info` says of `task`; whether that changed. */
    private static boolean update(RunningTaskInfo info, Task task) {
        int activityType = info.getActivityType();
        // A task without activities keeps its last top activity's.
        int orientation = info.topActivityInfo != null
                ? info.topActivityInfo.screenOrientation : task.orientation;
        boolean changed = activityType != task.activityType || orientation != task.orientation;
        task.activityType = activityType;
        task.orientation = orientation;
        return changed;
    }

    private void report(int taskId, Task task) {
        if (mWindows == null) {
            return;
        }
        try {
            mWindows.onTaskChanged(taskId, task.activityType, task.orientation);
        } catch (RemoteException e) {
            // Dead: its death notice detaches it.
        }
    }

    /**
     * In window mode, whether `task`, opening or coming to the front
     * (`mode`), is a fullscreen app task on the default display, which
     * becomes freeform: it takes the freeform bounds its launch gave it as
     * restore bounds, as WMShell's desktop mode does with a fullscreen
     * launch.
     */
    private boolean becomesFreeform(RunningTaskInfo task, int mode) {
        return mWindows != null && task != null && task.displayId == DEFAULT_DISPLAY
                && (mode == WindowManager.TRANSIT_OPEN || mode == WindowManager.TRANSIT_TO_FRONT)
                && task.getActivityType() == WindowConfiguration.ACTIVITY_TYPE_STANDARD
                && task.getWindowingMode() == WindowConfiguration.WINDOWING_MODE_FULLSCREEN;
    }

    private static WindowContainerTransaction freeform(List<RunningTaskInfo> tasks) {
        WindowContainerTransaction wct = new WindowContainerTransaction();
        for (RunningTaskInfo task : tasks) {
            wct.setWindowingMode(task.token, WindowConfiguration.WINDOWING_MODE_FREEFORM);
        }
        return wct;
    }

    private final class Player extends ITransitionPlayer.Stub {
        @Override
        public void requestStartTransition(IBinder token, TransitionRequestInfo request) {
            mHandler.post(() -> {
                WindowContainerTransaction wct = null;
                try {
                    RunningTaskInfo task = request.getTriggerTask();
                    if (becomesFreeform(task, request.getType())) {
                        wct = freeform(List.of(task));
                    }
                } finally {
                    startTransition(token, wct);
                }
            });
        }

        @Override
        public void onTransitionReady(IBinder token, TransitionInfo info,
                SurfaceControl.Transaction start, SurfaceControl.Transaction finish) {
            // WindowManager's own transactions, which it closes when the
            // transition finishes: taken over here, as a parcel would copy them.
            SurfaceControl.Transaction startT = new SurfaceControl.Transaction().merge(start);
            SurfaceControl.Transaction finishT = new SurfaceControl.Transaction().merge(finish);
            List<TransitionInfo.Change> changes = new ArrayList<>(info.getChanges());
            mHandler.post(() -> {
                List<RunningTaskInfo> late = new ArrayList<>();
                try {
                    // A task that opened in a transition requested for
                    // something else (a start while another collects) never
                    // had its own request.
                    for (TransitionInfo.Change change : changes) {
                        if (becomesFreeform(change.getTaskInfo(), change.getMode())) {
                            late.add(change.getTaskInfo());
                        }
                    }
                    startT.apply();
                    finishT.apply();
                } finally {
                    finishTransition(token, null);
                }
                if (!late.isEmpty()) {
                    startNewTransition(WindowManager.TRANSIT_CHANGE, freeform(late));
                }
            });
        }
    }

    private final class Service extends IWindowShell.Stub {
        @Override
        public void attach(IWindowShellListener listener) throws RemoteException {
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("the window shell serves the system uid only");
            }
            IBinder binder = listener.asBinder();
            binder.linkToDeath(() -> mHandler.post(() -> {
                if (mWindows != null && mWindows.asBinder() == binder) {
                    mWindows = null;
                }
            }), 0);
            mHandler.post(() -> {
                mWindows = listener;
                for (Map.Entry<Integer, Task> e : mTasks.entrySet()) {
                    report(e.getKey(), e.getValue());
                }
            });
        }
    }
}
