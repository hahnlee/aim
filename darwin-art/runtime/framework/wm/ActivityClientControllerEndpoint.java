package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import java.lang.reflect.Field;
import java.util.ArrayDeque;
import java.util.HashMap;

/** System-owned activity-token state exposed through IActivityClientController. */
public final class ActivityClientControllerEndpoint extends Binder {
    private enum State { RESUMED, PAUSED }

    private static final class ActivityRecord {
        final IBinder applicationThread;
        State state = State.RESUMED;

        ActivityRecord(IBinder applicationThread) {
            this.applicationThread = applicationThread;
        }
    }

    private static final Object activityLock = new Object();
    private static final HashMap<IBinder, ActivityRecord> activityRecords = new HashMap<>();
    private static final HashMap<IBinder, ArrayDeque<IBinder>> activityStacks = new HashMap<>();
    private final int getDisplayIdCode = transaction("getDisplayId");
    private final int activityResumedCode = transaction("activityResumed");
    private final int activityPausedCode = transaction("activityPaused");
    private final int activityDestroyedCode = transaction("activityDestroyed");
    private final int finishActivityCode = transaction("finishActivity");

    public ActivityClientControllerEndpoint() {
        attachInterface(null, "android.app.IActivityClientController");
    }

    public static void registerActivityToken(IBinder applicationThread, IBinder token) {
        if (applicationThread == null || token == null) {
            throw new IllegalArgumentException("Activity application thread/token is null");
        }
        synchronized (activityLock) {
            commitLaunchLocked(applicationThread, token);
        }
    }

    IBinder topActivityToken(IBinder applicationThread) {
        synchronized (activityLock) {
            ArrayDeque<IBinder> stack = activityStacks.get(applicationThread);
            return stack == null ? null : stack.peekLast();
        }
    }

    void commitLaunch(IBinder applicationThread, IBinder token) {
        synchronized (activityLock) {
            IBinder previous = topActivityTokenLocked(applicationThread);
            if (previous != null) activityRecords.get(previous).state = State.PAUSED;
            commitLaunchLocked(applicationThread, token);
        }
    }

    private static void commitLaunchLocked(IBinder applicationThread, IBinder token) {
        if (activityRecords.containsKey(token)) return;
        activityRecords.put(token, new ActivityRecord(applicationThread));
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
                && code != finishActivityCode) {
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
                    if (previous != null) activityRecords.get(previous).state = State.RESUMED;
                }
            }
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
