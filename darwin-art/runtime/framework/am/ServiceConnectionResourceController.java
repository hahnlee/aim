package dev.darwinart.runtime.am;

import java.util.ArrayList;
import java.util.IdentityHashMap;

/**
 * Owns deferred Binder death-registration resource tails for ActiveServices.
 *
 * The supplied monitor is still the ActiveServices policy lock.  Queue
 * mutations are made only while that monitor is held; unlinkToDeath runs only
 * after the monitor has been released.  A failed tail is retained for a later
 * drain and is never converted into a successful cleanup result.
 */
final class ServiceConnectionResourceController {
    private final Object monitor;
    private final ArrayList<ServiceConnectionDeathRegistration> pending = new ArrayList<>();

    ServiceConnectionResourceController(Object activeServicesMonitor) {
        if (activeServicesMonitor == null) {
            throw new IllegalArgumentException("Missing ActiveServices monitor");
        }
        monitor = activeServicesMonitor;
    }

    /** Caller owns the supplied ActiveServices monitor. */
    void enqueueLocked(ServiceConnectionDeathRegistration registration) {
        if (!Thread.holdsLock(monitor)) {
            throw new IllegalStateException("Death-registration queue requires ActiveServices lock");
        }
        if (registration == null) throw new IllegalArgumentException("Missing death registration");
        pending.add(registration);
    }

    Throwable drain() {
        return drain(new IdentityHashMap<>());
    }

    /**
     * Drains each registration at most once for this outer drain cycle.  A
     * caller may reuse the identity map while draining other tails; failed
     * registrations remain queued but are not retried recursively in the same
     * cycle.
     */
    Throwable drain(IdentityHashMap<ServiceConnectionDeathRegistration, Boolean> tried) {
        if (tried == null) throw new IllegalArgumentException("Missing drain identity set");
        if (Thread.holdsLock(monitor)) return null;

        ArrayList<ServiceConnectionDeathRegistration> tails;
        synchronized (monitor) {
            if (pending.isEmpty()) return null;
            tails = new ArrayList<>();
            for (ServiceConnectionDeathRegistration registration : pending) {
                if (tried.put(registration, Boolean.TRUE) == null) tails.add(registration);
            }
            pending.removeAll(tails);
        }

        Throwable firstFailure = null;
        for (ServiceConnectionDeathRegistration registration : tails) {
            try {
                registration.unlinkOutsideLock();
            } catch (RuntimeException | Error failure) {
                synchronized (monitor) {
                    pending.add(registration);
                }
                if (firstFailure == null) firstFailure = failure;
                else if (firstFailure != failure) firstFailure.addSuppressed(failure);
            }
        }
        return firstFailure;
    }
}
