package dev.darwinart.runtime.wm;

import android.os.Handler;
import android.os.HandlerThread;

/** Binder delivery scheduler only; a blocked client never occupies the WMS policy Looper. */
final class WindowRootFocusDecisionDelivery {
    private final Handler handler;

    WindowRootFocusDecisionDelivery() {
        HandlerThread thread = new HandlerThread("WindowManagerRootDecisionDelivery");
        thread.start();
        handler = new Handler(thread.getLooper());
    }

    void schedule(Runnable action, int delay) {
        if (!handler.postDelayed(action, delay))
            throw new IllegalStateException("root decision delivery Looper rejected work");
    }
}
