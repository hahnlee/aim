package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.view.View;
import android.view.WindowManager;
import java.lang.reflect.Field;
import java.util.HashMap;
import java.util.LinkedHashMap;

/** WMS owner for desktop metadata derived from Android top-level windows. */
public final class DesktopWindowMetadataRegistry {
    static final String RECEIVER_DESCRIPTOR =
            "dev.darwinart.runtime.wm.IDesktopWindowMetadataReceiver";
    static final int TRANSACTION_UPDATE = IBinder.FIRST_CALL_TRANSACTION;

    private static final Field ACCESSIBILITY_TITLE = accessibilityTitleField();

    private static final class WindowState {
        final IBinder token;
        String title;
        boolean visible;

        WindowState(IBinder window) {
            token = window;
        }
    }

    private static final class ProcessState {
        IBinder receiver;
        long generation;
        final LinkedHashMap<IBinder, WindowState> windows = new LinkedHashMap<>();
    }

    private final HashMap<Integer, ProcessState> processes = new HashMap<>();

    public synchronized void register(int pid, IBinder receiver) {
        if (pid <= 0 || receiver == null) throw new IllegalArgumentException("invalid receiver");
        ProcessState process = processes.get(pid);
        if (process == null) {
            process = new ProcessState();
            processes.put(pid, process);
        }
        process.receiver = receiver;
        publishSelected(process);
    }

    synchronized void update(int pid, IBinder window, WindowManager.LayoutParams attrs,
            int visibility) {
        if (pid <= 0 || window == null || attrs == null) return;
        if (attrs.type < WindowManager.LayoutParams.FIRST_APPLICATION_WINDOW
                || attrs.type > WindowManager.LayoutParams.LAST_APPLICATION_WINDOW) return;
        ProcessState process = processes.get(pid);
        if (process == null) {
            process = new ProcessState();
            processes.put(pid, process);
        }
        WindowState state = process.windows.remove(window);
        if (state == null) state = new WindowState(window);
        state.title = accessibilityTitle(attrs);
        state.visible = visibility == View.VISIBLE;
        ++process.generation;
        process.windows.put(window, state);
        publishSelected(process);
    }

    synchronized void remove(int pid, IBinder window) {
        ProcessState process = processes.get(pid);
        if (process == null || process.windows.remove(window) == null) return;
        ++process.generation;
        publishSelected(process);
    }

    private static void publishSelected(ProcessState process) {
        if (process.receiver == null) return;
        WindowState selected = null;
        for (WindowState candidate : process.windows.values()) {
            if (candidate.visible && candidate.title != null && !candidate.title.isEmpty()) {
                selected = candidate;
            }
        }
        if (selected == null) return;
        Parcel data = Parcel.obtain();
        try {
            data.writeInterfaceToken(RECEIVER_DESCRIPTOR);
            data.writeStrongBinder(selected.token);
            data.writeLong(process.generation);
            data.writeString(selected.title);
            if (!process.receiver.transact(
                    TRANSACTION_UPDATE, data, null, IBinder.FLAG_ONEWAY)) {
                process.receiver = null;
            }
        } catch (RemoteException error) {
            // A dead application must not make a WMS add/relayout/remove transaction fail.
            process.receiver = null;
        } finally {
            data.recycle();
        }
    }

    private static String accessibilityTitle(WindowManager.LayoutParams attrs) {
        try {
            CharSequence title = (CharSequence) ACCESSIBILITY_TITLE.get(attrs);
            return title == null ? null : title.toString();
        } catch (IllegalAccessException error) {
            throw new IllegalStateException("cannot read WindowManager accessibilityTitle", error);
        }
    }

    private static Field accessibilityTitleField() {
        try {
            Field field = WindowManager.LayoutParams.class.getDeclaredField("accessibilityTitle");
            field.setAccessible(true);
            return field;
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }
}
