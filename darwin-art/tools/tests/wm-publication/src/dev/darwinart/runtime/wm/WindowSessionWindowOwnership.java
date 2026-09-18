package dev.darwinart.runtime.wm;

import android.os.IBinder;

/** Minimal ownership seam; production controller consumes only registration identity. */
final class WindowSessionWindowOwnership {
    static final class Registration {
        private final IBinder window;

        private Registration(IBinder value) { window = value; }

        IBinder window() { return window; }
    }

    static Registration registration(IBinder window) {
        if (window == null) throw new IllegalArgumentException("missing window");
        return new Registration(window);
    }
}
