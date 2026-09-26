package dev.darwinart.runtime.display;

import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.Log;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.concurrent.ConcurrentHashMap;

/**
 * DisplayManager owner for each desktop task's logical display 0.
 *
 * <p>Every macOS window owns a separate scanout, so each Android application
 * process observes its own task-sized display 0 (see ADR 0008). Geometry is
 * keyed by the exact process; a resize of one task never changes another
 * process's DisplayInfo. Reads are lock-free snapshots so WMS relayout and
 * DisplayManager queries never wait on a publication in progress.</p>
 */
public final class TaskDisplayRegistry {
    private static final String TAG = "DarwinTaskDisplay";
    private static final String CALLBACK_DESCRIPTOR =
            "android.hardware.display.IDisplayManagerCallback";

    /** Pinned framework constants, resolved only when a callback is used. */
    private static final class Events {
        static final int ON_DISPLAY_EVENT = callbackTransaction("onDisplayEvent");
        static final int DISPLAY_BASIC_CHANGED = globalConstant("EVENT_DISPLAY_BASIC_CHANGED");
        static final long FLAG_DISPLAY_BASIC_CHANGED =
                globalLongConstant("INTERNAL_EVENT_FLAG_DISPLAY_BASIC_CHANGED");
        static final long DEFAULT_MASK = globalLongConstant("INTERNAL_EVENT_FLAG_DISPLAY_ADDED")
                | FLAG_DISPLAY_BASIC_CHANGED
                | globalLongConstant("INTERNAL_EVENT_FLAG_DISPLAY_REMOVED");
    }

    private static final class Callback {
        final int pid;
        final IBinder binder;
        final long mask;
        IBinder.DeathRecipient death;

        Callback(int pid, IBinder binder, long mask) {
            this.pid = pid;
            this.binder = binder;
            this.mask = mask;
        }
    }

    private final ConcurrentHashMap<Integer, DisplayGeometry> geometries =
            new ConcurrentHashMap<>();
    // CGDirectDisplayID of the macOS display each task's root is on.
    private final ConcurrentHashMap<Integer, Integer> hostDisplays = new ConcurrentHashMap<>();
    private final Object callbackLock = new Object();
    private final HashMap<Integer, ArrayList<Callback>> callbacks = new HashMap<>();

    /** The macOS display the process's root is on, or 0 when unknown. */
    public int hostDisplay(int pid) {
        return hostDisplays.getOrDefault(pid, 0);
    }

    /** Records the root's macOS display; true when it changed. */
    public boolean storeHostDisplay(int pid, int displayId) {
        if (pid <= 0 || displayId == 0) return false;
        Integer previous = hostDisplays.put(pid, displayId);
        boolean moved = previous == null || previous != displayId;
        if (moved) {
            HostDisplayFacts facts = HostDisplayFacts.describe(displayId);
            Log.i(TAG, "task display pid=" + pid + " host=" + displayId + " "
                    + (facts == null ? "unknown" : facts.name + (facts.builtIn ? " built-in" : "")
                            + " dpi=" + facts.xDpi + "x" + facts.yDpi));
        }
        return moved;
    }

    /** The logical display 0 geometry observed by one process. */
    public DisplayGeometry geometry(int pid) {
        DisplayGeometry geometry = geometries.get(pid);
        return geometry == null ? DisplayGeometry.initial() : geometry;
    }

    /**
     * Publishes a task geometry and notifies that process's registered display
     * clients so DisplayManagerGlobal refreshes its cached DisplayInfo. All
     * callbacks are one-way; no caller monitor is held across delivery.
     */
    public void publish(int pid, DisplayGeometry geometry) {
        if (store(pid, geometry)) notifyChanged(pid);
    }

    /** Makes {@code geometry} current for DisplayInfo queries; true if the extent changed. */
    public boolean store(int pid, DisplayGeometry geometry) {
        if (pid <= 0 || geometry == null) throw new IllegalArgumentException("invalid display publication");
        DisplayGeometry previous = geometries.put(pid, geometry);
        return previous == null || !previous.sameExtent(geometry.widthPixels, geometry.heightPixels)
                || previous.densityDpi != geometry.densityDpi;
    }

    /** Sends EVENT_DISPLAY_BASIC_CHANGED to the process's display callbacks. */
    public void notifyChanged(int pid) {
        ArrayList<Callback> targets;
        synchronized (callbackLock) {
            ArrayList<Callback> registered = callbacks.get(pid);
            targets = registered == null ? new ArrayList<Callback>() : new ArrayList<>(registered);
        }
        for (Callback callback : targets) {
            if ((callback.mask & Events.FLAG_DISPLAY_BASIC_CHANGED) == 0) continue;
            Parcel data = Parcel.obtain();
            try {
                data.writeInterfaceToken(CALLBACK_DESCRIPTOR);
                data.writeInt(0); // Display.DEFAULT_DISPLAY of this task.
                data.writeInt(Events.DISPLAY_BASIC_CHANGED);
                callback.binder.transact(Events.ON_DISPLAY_EVENT, data, null, IBinder.FLAG_ONEWAY);
            } catch (RemoteException error) {
                unregister(callback);
            } finally {
                data.recycle();
            }
        }
    }

    public void registerCallback(int pid, IBinder binder, long mask) throws RemoteException {
        if (pid <= 0 || binder == null) throw new IllegalArgumentException("invalid display callback");
        Callback callback = new Callback(pid, binder, mask == 0 ? Events.DEFAULT_MASK : mask);
        synchronized (callbackLock) {
            ArrayList<Callback> registered = callbacks.get(pid);
            if (registered == null) {
                registered = new ArrayList<>();
                callbacks.put(pid, registered);
            }
            for (Callback existing : registered) {
                // DisplayManagerGlobal registers once per process; a repeated
                // registration only replaces its event mask.
                if (existing.binder == binder) {
                    registered.remove(existing);
                    if (existing.death != null) binder.unlinkToDeath(existing.death, 0);
                    break;
                }
            }
            callback.death = () -> unregister(callback);
            binder.linkToDeath(callback.death, 0);
            registered.add(callback);
        }
    }

    /** Process retirement: forget geometry and callbacks of that exact process. */
    public void removeProcess(int pid) {
        geometries.remove(pid);
        hostDisplays.remove(pid);
        ArrayList<Callback> removed;
        synchronized (callbackLock) {
            removed = callbacks.remove(pid);
        }
        if (removed == null) return;
        for (Callback callback : removed) {
            if (callback.death != null) callback.binder.unlinkToDeath(callback.death, 0);
        }
    }

    private void unregister(Callback callback) {
        synchronized (callbackLock) {
            ArrayList<Callback> registered = callbacks.get(callback.pid);
            if (registered != null && registered.remove(callback) && registered.isEmpty()) {
                callbacks.remove(callback.pid);
            }
        }
        Log.i(TAG, "display callback retired pid=" + callback.pid);
    }

    private static int callbackTransaction(String name) {
        try {
            Field field = Class.forName("android.hardware.display.IDisplayManagerCallback$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static Field globalField(String name) throws ReflectiveOperationException {
        Field field = Class.forName("android.hardware.display.DisplayManagerGlobal")
                .getDeclaredField(name);
        field.setAccessible(true);
        return field;
    }

    private static int globalConstant(String name) {
        try {
            return globalField(name).getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static long globalLongConstant(String name) {
        try {
            return globalField(name).getLong(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }
}
