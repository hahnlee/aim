package dev.darwinart.runtime.wm;

import android.content.pm.ActivityInfo;
import android.content.res.Configuration;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import java.lang.reflect.Field;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;

/** System-owned activity-token state exposed through IActivityClientController. */
public final class ActivityClientControllerEndpoint extends Binder {
    private enum State { RESUMED, PAUSED }

    private static final class ActivityRecord {
        final IBinder applicationThread;
        final ActivityInfo info;
        State state = State.RESUMED;
        int requestedOrientation;
        // Merged configuration most recently sent to the Activity by a launch,
        // configuration change or relaunch transaction.
        Configuration reported;

        ActivityRecord(IBinder applicationThread, ActivityInfo info, Configuration reported) {
            this.applicationThread = applicationThread;
            this.info = info;
            this.requestedOrientation = info == null
                    ? ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED : info.screenOrientation;
            this.reported = reported == null ? null : new Configuration(reported);
        }
    }

    /** Immutable view of one live Activity for task configuration dispatch. */
    static final class ActivitySnapshot {
        final IBinder token;
        final boolean resumed;
        final int configChanges;
        final int targetSdkVersion;
        final Configuration reported;

        ActivitySnapshot(IBinder token, ActivityRecord record) {
            this.token = token;
            resumed = record.state == State.RESUMED;
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
    private final int activityResumedCode = transaction("activityResumed");
    private final int activityPausedCode = transaction("activityPaused");
    private final int activityDestroyedCode = transaction("activityDestroyed");
    private final int activityRelaunchedCode = transaction("activityRelaunched");
    private final int finishActivityCode = transaction("finishActivity");
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
        synchronized (activityLock) {
            commitLaunchLocked(applicationThread, token, info, reported);
        }
    }

    IBinder topActivityToken(IBinder applicationThread) {
        synchronized (activityLock) {
            return topActivityTokenLocked(applicationThread);
        }
    }

    void commitLaunch(IBinder applicationThread, IBinder token, ActivityInfo info,
            Configuration reported) {
        synchronized (activityLock) {
            IBinder previous = topActivityTokenLocked(applicationThread);
            if (previous != null) activityRecords.get(previous).state = State.PAUSED;
            commitLaunchLocked(applicationThread, token, info, reported);
        }
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
    }

    private static void commitLaunchLocked(IBinder applicationThread, IBinder token,
            ActivityInfo info, Configuration reported) {
        if (activityRecords.containsKey(token)) return;
        activityRecords.put(token, new ActivityRecord(applicationThread, info, reported));
        activityStacks.computeIfAbsent(applicationThread, unused -> new ArrayDeque<>())
                .addLast(token);
    }

    private static IBinder topActivityTokenLocked(IBinder applicationThread) {
        ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
        return stack == null ? null : stack.peekLast();
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
                && code != activityResumedCode
                && code != activityPausedCode
                && code != activityDestroyedCode
                && code != activityRelaunchedCode
                && code != finishActivityCode
                && code != setRequestedOrientationCode
                && code != getRequestedOrientationCode) {
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface("android.app.IActivityClientController");
        IBinder token = data.readStrongBinder();
        if (code == activityResumedCode) {
            data.readBoolean(); // handleSplashScreenExit
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                requireActivityLocked(token).state = State.RESUMED;
            }
            return true;
        }
        if (code == activityPausedCode) {
            data.enforceNoDataAvail();
            synchronized (activityLock) {
                requireActivityLocked(token).state = State.PAUSED;
            }
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
            return true;
        }
        if (code == finishActivityCode) {
            data.readInt(); // Result code.
            data.readTypedObject(android.content.Intent.CREATOR); // Result data.
            data.readInt(); // finishTask mode.
            data.enforceNoDataAvail();
            boolean scheduled;
            IBinder revealedThread = null;
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
                        revealed.state = State.RESUMED;
                        revealedThread = record.applicationThread;
                        revealedOrientation = revealed.requestedOrientation;
                    }
                }
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
