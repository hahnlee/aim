package dev.aim.server;

import android.app.ActivityManager.RunningTaskInfo;
import android.app.WindowConfiguration;
import android.content.Context;
import android.content.pm.ActivityInfo;
import android.graphics.Rect;
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
import android.window.StartingWindowInfo;
import android.window.StartingWindowRemovalInfo;
import android.window.TaskAppearedInfo;
import android.window.TaskOrganizer;
import android.window.TransitionInfo;
import android.window.TransitionRequestInfo;
import android.window.WindowContainerToken;
import android.window.WindowContainerTransaction;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The device's window shell in place of WMShell (docs/task-organizer.md):
 * the task organizer and the transition player, which make window mode
 * Android's desktop windowing. The default display stays fullscreen and
 * its tasks are freeform, so a start into a running task keeps its bounds
 * (#613). The player applies each transition at once, as WindowManager
 * does when a player fails; in window mode it makes a new fullscreen task
 * freeform in its transition's request, unless its start asked for a
 * windowing mode (LaunchModes), and tells aim-windows of each
 * task that took part in a transition once its surface is in place.
 * The organizer tells aim-windows each task's activity type and its top
 * activity's orientation and its windowing mode (#593), and draws the
 * tasks' starting windows (StartingWindows). A task entering
 * picture-in-picture gets its PiP bounds and mode (PipBounds), as WMShell's
 * PiP transition gives them.
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

    private final Context mContext;
    private final Handler mHandler;
    private final StartingWindows mStartingWindows;
    /** The organized tasks; on the shell's thread only. */
    private final Map<Integer, Task> mTasks = new HashMap<>();
    /** The windowing modes starts asked for. */
    private final LaunchModes mLaunchModes;
    /** The tasks whose start asked for a windowing mode, which they keep. */
    private final Set<Integer> mAsLaunched = new HashSet<>();
    /**
     * Starting windows held until their task is freeform: one added while
     * the task is still fullscreen would be resized by the conversion, and
     * WindowManager does not hand a resized splash screen to its app
     * (`StartingData.mResizedFromTransfer`).
     */
    private final Map<Integer, StartingWindowInfo> mHeldStarts = new HashMap<>();
    /** The tasks in PiP the shell placed, with their aspect ratio. */
    private final Map<Integer, Float> mPip = new HashMap<>();
    /** aim-windows in window mode, null otherwise; on the shell's thread only. */
    private IWindowShellListener mWindows;

    private static final class Task {
        final SurfaceControl leash;
        int activityType;
        int orientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
        int windowingMode;
        WindowContainerToken token;

        Task(SurfaceControl leash) {
            this.leash = leash;
        }
    }

    private WindowShell(Context context, Handler handler, LaunchModes launchModes) {
        super(null, handler::post);
        mContext = context;
        mLaunchModes = launchModes;
        mHandler = handler;
        mStartingWindows = new StartingWindows(context, handler);
    }

    /** Registers the organizer and the player, and publishes the shell. */
    static void start(Context context) {
        HandlerThread thread = new HandlerThread(TAG);
        thread.start();
        LaunchModes launchModes = new LaunchModes();
        ProductInterceptor.add(launchModes);
        WindowShell shell = new WindowShell(context, thread.getThreadHandler(), launchModes);
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
        Float ratio = mPip.get(info.taskId);
        if (ratio == null) {
            return;
        }
        if (info.getWindowingMode() != WindowConfiguration.WINDOWING_MODE_PINNED) {
            mPip.remove(info.taskId);
            return;
        }
        // The app asked for another aspect ratio (setPictureInPictureParams).
        float now = PipBounds.aspectRatio(info, mContext.getResources());
        if (now != ratio) {
            mPip.put(info.taskId, now);
            startNewTransition(WindowManager.TRANSIT_CHANGE, new WindowContainerTransaction()
                    .setBounds(info.token, PipBounds.bounds(info, now,
                            info.configuration.windowConfiguration.getBounds())));
        }
    }

    @Override
    public void onTaskVanished(RunningTaskInfo info) {
        Task task = mTasks.remove(info.taskId);
        if (task != null) {
            task.leash.release();
        }
        mPip.remove(info.taskId);
        mAsLaunched.remove(info.taskId);
        mHeldStarts.remove(info.taskId);
    }

    @Override
    public void addStartingWindow(StartingWindowInfo info) {
        RunningTaskInfo task = info.taskInfo;
        if (willBecomeFreeform(task)) {
            mHeldStarts.put(task.taskId, info);
        } else {
            mStartingWindows.add(info);
        }
    }

    /** The starting window held for `taskId`, now that its task is freeform. */
    private void releaseStart(int taskId) {
        StartingWindowInfo info = mHeldStarts.remove(taskId);
        if (info != null) {
            mStartingWindows.add(info);
        }
    }

    @Override
    public void removeStartingWindow(StartingWindowRemovalInfo info) {
        // One still held was never shown.
        if (mHeldStarts.remove(info.taskId) == null) {
            mStartingWindows.remove(info);
        }
    }

    @Override
    public void copySplashScreenView(int taskId) {
        mStartingWindows.copy(taskId);
    }

    @Override
    public void onAppSplashScreenViewRemoved(int taskId) {
        mStartingWindows.appRemoved(taskId);
    }

    /** Takes what `info` says of `task`; whether that changed. */
    private static boolean update(RunningTaskInfo info, Task task) {
        int activityType = info.getActivityType();
        // A task without activities keeps its last top activity's.
        int orientation = info.topActivityInfo != null
                ? info.topActivityInfo.screenOrientation : task.orientation;
        int windowingMode = info.getWindowingMode();
        boolean changed = activityType != task.activityType || orientation != task.orientation
                || windowingMode != task.windowingMode;
        task.activityType = activityType;
        task.orientation = orientation;
        task.windowingMode = windowingMode;
        task.token = info.token;
        return changed;
    }

    private void report(int taskId, Task task) {
        if (mWindows == null) {
            return;
        }
        try {
            mWindows.onTaskChanged(taskId, task.activityType, task.orientation,
                    task.windowingMode);
        } catch (RemoteException e) {
            // Dead: its death notice detaches it.
        }
    }

    private void reportPlaced(int taskId) {
        if (mWindows == null) {
            return;
        }
        try {
            mWindows.onTaskPlaced(taskId);
        } catch (RemoteException e) {
            // Dead: its death notice detaches it.
        }
    }

    /**
     * In window mode, whether `task`, opening or coming to the front
     * (`mode`), is a fullscreen app task on the default display, which
     * becomes freeform: it takes the freeform bounds its launch gave it as
     * restore bounds, as WMShell's desktop mode does with a fullscreen
     * launch. A task whose start asked for a windowing mode keeps it, as
     * on a freeform display area.
     */
    private boolean becomesFreeform(RunningTaskInfo task, int mode) {
        if (mWindows == null || task == null || task.displayId != DEFAULT_DISPLAY
                || (mode != WindowManager.TRANSIT_OPEN && mode != WindowManager.TRANSIT_TO_FRONT)
                || task.getActivityType() != WindowConfiguration.ACTIVITY_TYPE_STANDARD
                || task.getWindowingMode() != WindowConfiguration.WINDOWING_MODE_FULLSCREEN) {
            return false;
        }
        if (mode == WindowManager.TRANSIT_OPEN && task.topActivity != null
                && mLaunchModes.take(task.topActivity)
                        != WindowConfiguration.WINDOWING_MODE_UNDEFINED) {
            mAsLaunched.add(task.taskId);
        }
        return !mAsLaunched.contains(task.taskId);
    }

    /**
     * Whether `task`, opening, will become freeform (becomesFreeform) once
     * its transition is requested.
     */
    private boolean willBecomeFreeform(RunningTaskInfo task) {
        return mWindows != null && task.displayId == DEFAULT_DISPLAY
                && task.getActivityType() == WindowConfiguration.ACTIVITY_TYPE_STANDARD
                && task.getWindowingMode() == WindowConfiguration.WINDOWING_MODE_FULLSCREEN
                && !mAsLaunched.contains(task.taskId)
                && (task.topActivity == null || mLaunchModes.peek(task.topActivity)
                        == WindowConfiguration.WINDOWING_MODE_UNDEFINED);
    }

    /** Whether `task` was moved into PiP without the shell placing it yet. */
    private boolean entersPip(RunningTaskInfo task) {
        return task != null && !mPip.containsKey(task.taskId)
                && task.getWindowingMode() == WindowConfiguration.WINDOWING_MODE_PINNED;
    }

    /**
     * `task`, moved into a pinned root task by WindowManager, in PiP: its
     * PiP bounds, and its activity in the root task's pinned mode, which
     * tells the app (WMShell's PipTransition.augmentRequest).
     */
    private WindowContainerTransaction pip(WindowContainerTransaction wct, RunningTaskInfo task) {
        float ratio = PipBounds.aspectRatio(task, mContext.getResources());
        mPip.put(task.taskId, ratio);
        return wct.setActivityWindowingMode(task.token, WindowConfiguration.WINDOWING_MODE_UNDEFINED)
                .setBounds(task.token, PipBounds.bounds(task, ratio, null));
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
                    } else if (request.getType() == WindowManager.TRANSIT_PIP && entersPip(task)) {
                        wct = pip(new WindowContainerTransaction(), task);
                    }
                } finally {
                    startTransition(token, wct);
                }
                RunningTaskInfo task = request.getTriggerTask();
                if (task != null) {
                    releaseStart(task.taskId);
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
                List<RunningTaskInfo> latePip = new ArrayList<>();
                List<Integer> placed = new ArrayList<>();
                try {
                    // A task that opened in a transition requested for
                    // something else (a start while another collects) never
                    // had its own request; nor did one the system moved into
                    // PiP within another transition (auto-enter on leaving).
                    for (TransitionInfo.Change change : changes) {
                        RunningTaskInfo task = change.getTaskInfo();
                        if (becomesFreeform(task, change.getMode())) {
                            late.add(task);
                        } else if (entersPip(task)) {
                            latePip.add(task);
                        }
                        if (task != null) {
                            placed.add(task.taskId);
                        }
                    }
                    startT.apply();
                    finishT.apply();
                } finally {
                    finishTransition(token, null);
                }
                // Their surfaces are in place now: their windows follow
                // (#649), and show them.
                for (int taskId : placed) {
                    reportPlaced(taskId);
                }
                if (!late.isEmpty()) {
                    startNewTransition(WindowManager.TRANSIT_CHANGE, freeform(late));
                }
                for (TransitionInfo.Change change : changes) {
                    if (change.getTaskInfo() != null) {
                        releaseStart(change.getTaskInfo().taskId);
                    }
                }
                if (!latePip.isEmpty()) {
                    WindowContainerTransaction wct = new WindowContainerTransaction();
                    for (RunningTaskInfo task : latePip) {
                        pip(wct, task);
                    }
                    startNewTransition(WindowManager.TRANSIT_PIP, wct);
                }
            });
        }
    }

    private final class Service extends IWindowShell.Stub {
        @Override
        public void setPipBounds(int taskId, int left, int top, int right, int bottom) {
            enforceSystem();
            Rect bounds = new Rect(left, top, right, bottom);
            mHandler.post(() -> {
                Task task = mTasks.get(taskId);
                if (task != null && mPip.containsKey(taskId)) {
                    startNewTransition(WindowManager.TRANSIT_CHANGE,
                            new WindowContainerTransaction().setBounds(task.token, bounds));
                }
            });
        }

        private void enforceSystem() {
            if (Binder.getCallingUid() != Process.SYSTEM_UID) {
                throw new SecurityException("the window shell serves the system uid only");
            }
        }

        @Override
        public void attach(IWindowShellListener listener) throws RemoteException {
            enforceSystem();
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
