package dev.darwinart.runtime.am;

import android.os.IBinder;

/**
 * Immutable owner identity for one service connection registration.
 *
 * Application ownership is an exact process incarnation: PID, launch
 * sequence, application-thread Binder and UID must all match.  System-owned
 * registrations deliberately have no application identity and therefore
 * never match an application-death transition.
 */
final class ServiceConnectionOwner {
    private static final ServiceConnectionOwner SYSTEM = new ServiceConnectionOwner(
            true, 0, -1, null, -1);

    private final boolean system;
    private final int pid;
    private final long startSequence;
    private final IBinder thread;
    private final int uid;

    private ServiceConnectionOwner(
            boolean systemOwner, int processId, long sequence, IBinder applicationThread,
            int processUid) {
        system = systemOwner;
        pid = processId;
        startSequence = sequence;
        thread = applicationThread;
        uid = processUid;
    }

    static ServiceConnectionOwner system() {
        return SYSTEM;
    }

    static ServiceConnectionOwner application(
            ApplicationProcessRegistry.AttachedApplication attached) {
        if (attached == null || attached.pid <= 0 || attached.startSequence < 0
                || attached.thread == null || attached.uid < 0) {
            throw new IllegalArgumentException("Invalid application connection owner");
        }
        return new ServiceConnectionOwner(
                false, attached.pid, attached.startSequence, attached.thread, attached.uid);
    }

    boolean isSystem() {
        return system;
    }

    boolean isCurrent(ApplicationProcessRegistry registry) {
        return system || registry.hasCallerIncarnation(pid, startSequence, thread, uid);
    }

    boolean matches(ApplicationProcessRegistry.AttachedApplication attached) {
        return !system && attached != null
                && pid == attached.pid
                && startSequence == attached.startSequence
                && uid == attached.uid
                && thread != null
                && thread.equals(attached.thread);
    }

    boolean same(ServiceConnectionOwner other) {
        if (other == null || system != other.system) return false;
        return system || (pid == other.pid
                && startSequence == other.startSequence
                && uid == other.uid
                && thread.equals(other.thread));
    }
}
