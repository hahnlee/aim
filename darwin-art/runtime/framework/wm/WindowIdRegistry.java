package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.RemoteCallbackList;
import android.os.RemoteException;
import android.view.IWindowFocusObserver;
import android.view.IWindowId;
import java.util.IdentityHashMap;

/**
 * WindowState.mWindowId: each window's IWindowId (View.getWindowId), which
 * answers isFocused and notifies focus observers when WMS focus moves to or
 * from the window. Focus follows the focus publications the input owner
 * delivered, the same decisions the window itself receives.
 */
final class WindowIdRegistry implements WindowFocusPublicationDelivery.FocusListener {
    private static final class WindowId extends IWindowId.Stub {
        private final IBinder window;
        private final RemoteCallbackList<IWindowFocusObserver> observers =
                new RemoteCallbackList<>();
        private boolean focused;

        WindowId(IBinder window, boolean focused) {
            this.window = window;
            this.focused = focused;
        }

        @Override
        public synchronized boolean isFocused() {
            return focused;
        }

        @Override
        public void registerFocusObserver(IWindowFocusObserver observer) {
            if (observer != null) observers.register(observer);
        }

        @Override
        public void unregisterFocusObserver(IWindowFocusObserver observer) {
            if (observer != null) observers.unregister(observer);
        }

        void update(boolean value) {
            synchronized (this) {
                if (focused == value) return;
                focused = value;
            }
            synchronized (observers) {
                int count = observers.beginBroadcast();
                try {
                    for (int i = 0; i < count; i++) {
                        try {
                            if (value) {
                                observers.getBroadcastItem(i).focusGained(window);
                            } else {
                                observers.getBroadcastItem(i).focusLost(window);
                            }
                        } catch (RemoteException ignored) {
                            // A dead observer leaves the callback list.
                        }
                    }
                } finally {
                    observers.finishBroadcast();
                }
            }
        }

        void kill() {
            observers.kill();
        }
    }

    private final WindowFocusRegistry registry;
    private final IdentityHashMap<Object, WindowId> ids = new IdentityHashMap<>();

    WindowIdRegistry(WindowFocusRegistry registry) {
        this.registry = registry;
    }

    /** The IWindowId of the window whose WMS token is {@code token}. */
    synchronized IWindowId windowId(Object token, IBinder window) {
        WindowId id = ids.get(token);
        if (id == null) {
            WindowFocusRegistry.WindowSpec focused = registry.focusedWindow(0);
            id = new WindowId(window, focused != null && focused.windowToken == token);
            ids.put(token, id);
        }
        return id;
    }

    void remove(Object token) {
        WindowId id;
        synchronized (this) {
            id = ids.remove(token);
        }
        if (id != null) id.kill();
    }

    @Override
    public void focusChanged(Object token, boolean focused) {
        WindowId id;
        synchronized (this) {
            id = ids.get(token);
        }
        if (id != null) id.update(focused);
    }
}
