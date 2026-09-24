package dev.darwinart.runtime.wm;

import android.app.servertransaction.ClientTransactionItem;
import android.content.pm.ActivityInfo;
import android.content.res.Configuration;
import android.content.res.Resources;
import android.graphics.Rect;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.Log;
import android.view.WindowManager;
import dev.darwinart.runtime.am.ActivityManagerEndpoint;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.display.BuiltInDisplayConfiguration;
import dev.darwinart.runtime.display.DisplayGeometry;
import dev.darwinart.runtime.display.TaskDisplayRegistry;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;

/**
 * ActivityTask owner of each desktop task's geometry.
 *
 * <p>A task is one attached application process and its desktop root. Every
 * extent change (launch orientation, requested orientation, host resize)
 * produces one revision that is published in a fixed order: DisplayManager
 * (DisplayInfo + callbacks), then one ClientTransaction carrying the process
 * configuration, Activity configuration or relaunch items and a
 * WindowStateResizeItem per window, then the host root. Only one-way Binder
 * calls happen under the controller lock, so no caller monitor is held across
 * application or AppKit work. State is keyed by the exact application thread:
 * a closed, dead or recycled-PID task never receives another task's revision.</p>
 */
public final class TaskGeometryController implements ActivityManagerEndpoint.TaskLifecycle {
    private static final String TAG = "DarwinTaskGeometry";
    static final String HOST_RECEIVER_DESCRIPTOR =
            "dev.darwinart.runtime.wm.IDesktopRootGeometryReceiver";
    static final int HOST_PUBLISH = IBinder.FIRST_CALL_TRANSACTION;
    static final int HOST_STATUS_APPLIED = 0;

    private static volatile TaskGeometryController instance;

    private static final class Task {
        final int pid;
        final IBinder thread;
        DisplayGeometry geometry;
        IBinder hostReceiver;
        long hostSerial;
        long hostAppliedRevision = -1;
        boolean hostClosed;

        Task(int pid, IBinder thread, DisplayGeometry geometry) {
            this.pid = pid;
            this.thread = thread;
            this.geometry = geometry;
        }
    }

    private final ApplicationProcessRegistry processes;
    private final TaskDisplayRegistry displays;
    private final WindowSurfaceRegistry windows;
    private final HashMap<Integer, Task> tasks = new HashMap<>();
    // Revisions triggered by an application's synchronous call are published
    // from this thread, never from the Binder thread serving that call: a
    // one-way call made there would be delivered nested on the caller's
    // waiting thread, where the app's handler cannot call back out.
    private final Handler dispatcher;

    TaskGeometryController(ApplicationProcessRegistry processes, TaskDisplayRegistry displays,
            WindowSurfaceRegistry windows) {
        if (processes == null || displays == null || windows == null) throw new NullPointerException();
        this.processes = processes;
        this.displays = displays;
        this.windows = windows;
        HandlerThread thread = new HandlerThread("DarwinTaskGeometry");
        thread.start();
        dispatcher = new Handler(thread.getLooper());
    }

    private void dispatch(Runnable work) {
        if (!dispatcher.post(work)) throw new IllegalStateException("task geometry dispatcher stopped");
    }

    /** ActivityTask owner joined to {@code windowManager}'s window layout. */
    public static TaskGeometryController create(ApplicationProcessRegistry processes,
            TaskDisplayRegistry displays, WindowManagerEndpoint windowManager) {
        return new TaskGeometryController(processes, displays, windowManager.surfaces());
    }

    /** Publishes the system process's single controller for native launch/bind paths. */
    public void install() {
        synchronized (TaskGeometryController.class) {
            if (instance != null && instance != this) {
                throw new IllegalStateException("task geometry controller already installed");
            }
            instance = this;
        }
    }

    static TaskGeometryController requireInstance() {
        TaskGeometryController current = instance;
        if (current == null) throw new IllegalStateException("task geometry controller missing");
        return current;
    }

    /**
     * AMS attach, before bindApplication: creates this exact process's task and
     * resolves its launch Activity's orientation so Application and Activity
     * start with the same configuration.
     */
    @Override
    public void prepareProcess(int pid, IBinder thread, ActivityInfo launchActivity) {
        if (pid <= 0 || thread == null) throw new IllegalArgumentException("invalid task process");
        synchronized (this) {
            DisplayGeometry initial = DisplayGeometry.initial();
            int shape = launchActivity == null ? TaskGeometryPolicy.NONE
                    : TaskGeometryPolicy.requiredShape(launchActivity.screenOrientation);
            int[] extent = TaskGeometryPolicy.extentFor(
                    initial.widthPixels, initial.heightPixels, shape);
            Task task = new Task(pid, thread, new DisplayGeometry(
                    1, extent[0], extent[1], initial.densityDpi));
            tasks.put(pid, task);
            displays.publish(pid, task.geometry);
            Log.i(TAG, "task prepared pid=" + pid + " geometry=" + task.geometry
                    + " launchOrientation="
                    + (launchActivity == null ? "none" : launchActivity.screenOrientation));
        }
    }

    /** Process retirement from the AMS death owner. Late publications are dropped. */
    @Override
    public void removeProcess(int pid, IBinder thread) {
        synchronized (this) {
            Task task = tasks.get(pid);
            if (task == null || task.thread != thread) return;
            tasks.remove(pid);
        }
        displays.removeProcess(pid);
        ActivityClientControllerEndpoint.removeProcess(thread);
    }

    /** bindApplication configuration for the exact attaching application thread. */
    public static Configuration processConfiguration(IBinder thread, Configuration system) {
        TaskGeometryController controller = requireInstance();
        synchronized (controller) {
            Task task = controller.taskFor(thread);
            DisplayGeometry geometry = task == null ? DisplayGeometry.initial() : task.geometry;
            return geometry.configuration(system);
        }
    }

    /**
     * LaunchActivityItem configuration {global, override} for {@code info},
     * after applying its orientation to the task. Already running Activities and
     * windows of the task receive the same revision first.
     */
    public static Configuration[] launchConfiguration(IBinder thread, ActivityInfo info) {
        TaskGeometryController controller = requireInstance();
        synchronized (controller) {
            Task task = controller.taskFor(thread);
            if (task == null) {
                DisplayGeometry initial = DisplayGeometry.initial();
                return new Configuration[] {
                        initial.configuration(Resources.getSystem().getConfiguration()),
                        initial.overrideConfiguration()};
            }
            int shape = TaskGeometryPolicy.requiredShape(info.screenOrientation);
            int[] extent = TaskGeometryPolicy.extentFor(
                    task.geometry.widthPixels, task.geometry.heightPixels, shape);
            if (!task.geometry.sameExtent(extent[0], extent[1])) {
                controller.publishLocked(task, extent[0], extent[1], "launch " + info.name);
            }
            return new Configuration[] {
                    task.geometry.configuration(Resources.getSystem().getConfiguration()),
                    task.geometry.overrideConfiguration()};
        }
    }

    /** IActivityClientController.setRequestedOrientation on the task's top Activity. */
    void requestedOrientationChanged(IBinder thread, int requestedOrientation) {
        dispatch(() -> applyRequestedOrientation(thread, requestedOrientation));
    }

    private void applyRequestedOrientation(IBinder thread, int requestedOrientation) {
        synchronized (this) {
            Task task = taskFor(thread);
            if (task == null) return;
            int shape = TaskGeometryPolicy.requiredShape(requestedOrientation);
            int[] extent = TaskGeometryPolicy.extentFor(
                    task.geometry.widthPixels, task.geometry.heightPixels, shape);
            if (task.geometry.sameExtent(extent[0], extent[1])) return;
            publishLocked(task, extent[0], extent[1], "requestedOrientation "
                    + requestedOrientation);
        }
    }

    /** Desktop root receiver registration for the calling application process. */
    void registerHost(int pid, IBinder receiver) {
        if (receiver == null) throw new IllegalArgumentException("host receiver is null");
        ApplicationProcessRegistry.AttachedApplication attached =
                processes.requireAttachedProcess(pid);
        synchronized (this) {
            Task task = tasks.get(pid);
            if (task == null || task.thread != attached.thread) {
                throw new IllegalStateException("host root has no attached task");
            }
        }
        dispatch(() -> {
            synchronized (this) {
                Task task = tasks.get(pid);
                if (task == null || task.thread != attached.thread) return;
                task.hostReceiver = receiver;
                task.hostClosed = false;
                publishHostLocked(task);
            }
        });
    }

    /**
     * A user resize of the desktop root, in host content points. The window
     * owner chooses the task extent; Android pixels follow the Android raster
     * scale, not the host backing scale.
     */
    void hostResized(int pid, long serial, int pointsWidth, int pointsHeight) {
        dispatch(() -> applyHostResize(pid, serial, pointsWidth, pointsHeight));
    }

    private void applyHostResize(int pid, long serial, int pointsWidth, int pointsHeight) {
        synchronized (this) {
            Task task = tasks.get(pid);
            if (task == null || task.hostReceiver == null || task.hostClosed) return;
            if (serial <= task.hostSerial) return; // Stale or replayed host report.
            task.hostSerial = serial;
            int scale = BuiltInDisplayConfiguration.SCALE;
            int width = TaskGeometryPolicy.pixelsFromPoints(pointsWidth, scale);
            int height = TaskGeometryPolicy.pixelsFromPoints(pointsHeight, scale);
            if (!DisplayGeometry.validDimension(width) || !DisplayGeometry.validDimension(height)) {
                Log.w(TAG, "rejected host extent pid=" + pid + " points="
                        + pointsWidth + "x" + pointsHeight);
                return;
            }
            if (task.geometry.sameExtent(width, height)) {
                // Echo of a published extent: settle the host with the same revision.
                publishHostLocked(task);
                return;
            }
            publishLocked(task, width, height, "host resize serial=" + serial);
        }
    }

    /** Host application result for one revision; never mutates Android geometry. */
    void hostApplied(int pid, long revision, int status) {
        synchronized (this) {
            Task task = tasks.get(pid);
            if (task == null) return;
            if (status == HOST_STATUS_APPLIED) {
                task.hostAppliedRevision = Math.max(task.hostAppliedRevision, revision);
            } else if (status == 2 /* root closed */) {
                task.hostClosed = true;
            }
            Log.i(TAG, "host applied pid=" + pid + " revision=" + revision + " status=" + status
                    + " current=" + task.geometry.revision);
        }
    }

    private Task taskFor(IBinder thread) {
        for (Task task : tasks.values()) {
            if (task.thread == thread) return task;
        }
        return null;
    }

    private void publishLocked(Task task, int width, int height, String reason) {
        DisplayGeometry next = task.geometry.withExtent(task.geometry.revision + 1, width, height);
        task.geometry = next;
        Log.i(TAG, "publish pid=" + task.pid + " " + next + " reason=" + reason);
        // DisplayInfo, relayout and launch configuration read the new revision
        // immediately; the one-way deliveries leave from the dispatcher.
        boolean displayChanged = displays.store(task.pid, next);
        Runnable outbound = () -> {
            synchronized (this) {
                if (tasks.get(task.pid) != task) return; // Retired task.
                if (displayChanged) displays.notifyChanged(task.pid);
                try {
                    TaskClientTransactions.schedule(task.thread, transactionItems(task, next));
                } catch (RemoteException | RuntimeException error) {
                    // The process is dying or rejected the transaction; its death
                    // owner retires the task. Do not fabricate a delivered revision.
                    Log.w(TAG, "task transaction failed pid=" + task.pid + " " + next, error);
                }
                publishHostLocked(task);
            }
        };
        if (dispatcher.getLooper().isCurrentThread()) {
            outbound.run();
        } else {
            dispatch(outbound);
        }
    }

    private List<ClientTransactionItem> transactionItems(Task task, DisplayGeometry next) {
        Configuration global = next.configuration(Resources.getSystem().getConfiguration());
        Configuration override = next.overrideConfiguration();
        Configuration merged = new Configuration(global);
        merged.updateFrom(override);
        ArrayList<ClientTransactionItem> items = new ArrayList<>();
        items.add(TaskClientTransactions.processConfiguration(global));
        HashSet<IBinder> relaunched = new HashSet<>();
        for (ActivityClientControllerEndpoint.ActivitySnapshot activity
                : ActivityClientControllerEndpoint.activities(task.thread)) {
            int changes = activity.reported == null ? ~0
                    : TaskGeometryPolicy.reportableChanges(activity.reported.diff(merged));
            int handled = TaskGeometryPolicy.handledChanges(
                    activity.configChanges, activity.targetSdkVersion);
            if (changes != 0 && TaskGeometryPolicy.shouldRelaunch(changes, handled)) {
                TaskClientTransactions.relaunch(items, activity.token, changes, global, override,
                        activity.resumed);
                relaunched.add(activity.token);
            } else {
                items.add(TaskClientTransactions.activityConfiguration(activity.token, override));
            }
            ActivityClientControllerEndpoint.reported(activity.token, merged);
            Log.i(TAG, "activity " + (relaunched.contains(activity.token) ? "relaunch" : "config")
                    + " pid=" + task.pid + " revision=" + next.revision + " changes=0x"
                    + Integer.toHexString(changes) + " handled=0x" + Integer.toHexString(handled));
        }
        // Relaunched Activities remove and re-add their windows (and attached
        // popups); every other window is resized from this same snapshot.
        List<WindowSurfaceRegistry.TaskWindowFrame> frames = windows.taskFrames(task.pid, next);
        HashSet<IBinder> skipped = new HashSet<>();
        for (WindowSurfaceRegistry.TaskWindowFrame frame : frames) {
            if (!isSubWindow(frame.type) && relaunched.contains(frame.attachedToken)) {
                skipped.add(frame.window);
            }
        }
        Rect display = next.bounds();
        for (WindowSurfaceRegistry.TaskWindowFrame frame : frames) {
            if (skipped.contains(frame.window)
                    || (isSubWindow(frame.type) && skipped.contains(frame.attachedToken))) {
                continue;
            }
            items.add(TaskClientTransactions.windowResize(frame.window, frame.frame, display,
                    global, override, (int) next.revision));
        }
        return items;
    }

    private static boolean isSubWindow(int type) {
        return type >= WindowManager.LayoutParams.FIRST_SUB_WINDOW
                && type <= WindowManager.LayoutParams.LAST_SUB_WINDOW;
    }

    private void publishHostLocked(Task task) {
        if (task.hostReceiver == null || task.hostClosed) return;
        DisplayGeometry geometry = task.geometry;
        int scale = BuiltInDisplayConfiguration.SCALE;
        Parcel data = Parcel.obtain();
        try {
            data.writeInterfaceToken(HOST_RECEIVER_DESCRIPTOR);
            data.writeLong(geometry.revision);
            data.writeLong(task.hostSerial);
            data.writeInt(geometry.widthPixels);
            data.writeInt(geometry.heightPixels);
            data.writeInt(TaskGeometryPolicy.pointsFromPixels(geometry.widthPixels, scale));
            data.writeInt(TaskGeometryPolicy.pointsFromPixels(geometry.heightPixels, scale));
            task.hostReceiver.transact(HOST_PUBLISH, data, null, IBinder.FLAG_ONEWAY);
        } catch (RemoteException error) {
            task.hostReceiver = null;
            Log.w(TAG, "host root receiver lost pid=" + task.pid);
        } finally {
            data.recycle();
        }
    }
}
