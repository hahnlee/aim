package dev.darwinart.runtime.wm;

import android.os.IBinder;
import java.util.HashMap;

/** WMS-wide canonical IWindow registration, independent of channel/surface IO. */
final class WindowSessionWindowOwnership {
    private enum Phase { ADDING, LIVE, RETIRING }

    static final class Registration {
        private final Object owner;
        private final IBinder window;
        private Thread admittedThread;
        private Phase phase = Phase.ADDING;

        private Registration(Object owner, IBinder window) {
            this.owner = owner;
            this.window = window;
            admittedThread = Thread.currentThread();
        }

        IBinder window() { return window; }
    }

    private final HashMap<IBinder, Registration> windows = new HashMap<>();

    synchronized Registration claim(Object owner, IBinder window) {
        if (owner == null || window == null)
            throw new IllegalArgumentException("window registration requires session and token");
        if (windows.containsKey(window))
            throw new IllegalStateException("IWindow is already registered");
        Registration registration = new Registration(owner, window);
        windows.put(window, registration);
        return registration;
    }

    synchronized Registration require(Object owner, IBinder window) {
        Registration registration = windows.get(window);
        if (owner == null || registration == null || registration.owner != owner)
            throw new SecurityException("IWindow is not owned by this session");
        return registration;
    }

    /** Serializes the exact registration without holding a monitor during IO. */
    synchronized Registration begin(Object owner, IBinder window) {
        return admit(owner, window, false);
    }

    synchronized Registration beginCleanup(Object owner, IBinder window) {
        return admit(owner, window, true);
    }

    private Registration admit(Object owner, IBinder window, boolean cleanup) {
        Registration registration = require(owner, window);
        while (registration.admittedThread != null) {
            if (registration.admittedThread == Thread.currentThread())
                throw new IllegalStateException("reentrant IWindow operation");
            try {
                wait();
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException("interrupted IWindow admission", error);
            }
            // A waiter never adopts a successor that reuses the Binder token.
            if (windows.get(registration.window) != registration)
                throw new SecurityException("IWindow registration retired while waiting");
        }
        if (registration.phase != Phase.LIVE
                && !(cleanup && registration.phase == Phase.RETIRING))
            throw new IllegalStateException("IWindow is not live; only retirement cleanup is allowed");
        registration.admittedThread = Thread.currentThread();
        return registration;
    }

    synchronized void ready(Object owner, Registration registration) {
        requireAdmitted(owner, registration);
        if (registration.phase != Phase.ADDING)
            throw new IllegalStateException("IWindow add is no longer pending");
        registration.phase = Phase.LIVE;
    }

    synchronized void retire(Object owner, Registration registration) {
        requireAdmitted(owner, registration);
        registration.phase = Phase.RETIRING;
    }

    private void requireAdmitted(Object owner, Registration registration) {
        if (owner == null || registration == null || registration.owner != owner
                || windows.get(registration.window) != registration
                || registration.admittedThread != Thread.currentThread())
            throw new SecurityException("stale or foreign IWindow operation");
    }

    synchronized void release(Object owner, Registration registration) {
        requireAdmitted(owner, registration);
        if (registration.phase != Phase.RETIRING)
            throw new IllegalStateException("IWindow must retire before release");
        windows.remove(registration.window);
    }

    synchronized void end(Object owner, Registration registration) {
        if (owner == null || registration == null || registration.owner != owner
                || registration.admittedThread != Thread.currentThread())
            throw new SecurityException("foreign IWindow operation completion");
        // Completion concerns only the original record, even after release.
        registration.admittedThread = null;
        notifyAll();
    }
}
