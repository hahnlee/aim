package dev.darwinart.runtime.wm;

import android.os.IBinder;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** Per-session authentication owner for WMS callbacks from one app process. */
final class WindowSessionIdentity {
    private final ApplicationProcessRegistry registry;
    private final int pid;
    private final int uid;
    private final IBinder thread;
    private final long startSequence;

    WindowSessionIdentity(ApplicationProcessRegistry registry, int pid, int uid) {
        if (registry == null) throw new IllegalArgumentException("registry is null");
        ApplicationProcessRegistry.AttachedApplication checkedSnapshot =
                registry.requireIdentifiedAttachment(pid, uid);
        this.registry = registry;
        this.pid = pid;
        this.uid = uid;
        thread = checkedSnapshot.thread;
        startSequence = checkedSnapshot.startSequence;
    }

    /** Re-authenticates the current registry incarnation; snapshot identity is not reused. */
    void requireCaller(int callerPid, int callerUid) {
        if (callerPid != pid || callerUid != uid)
            throw new SecurityException("Window session caller identity mismatch");
        ApplicationProcessRegistry.AttachedApplication current =
                registry.requireIdentifiedAttachment(callerPid, callerUid);
        if (current.pid != pid || current.uid != uid || current.thread == null
                || !thread.equals(current.thread) || current.startSequence != startSequence) {
            throw new SecurityException("Window session attachment incarnation mismatch");
        }
    }

    int pid() {
        return pid;
    }

    int uid() {
        return uid;
    }

    /**
     * Compares the complete captured attachment, rather than just its PID/UID.
     *
     * <p>PID and UID are recyclable; the registry, application-thread Binder,
     * and start sequence together identify one attachment incarnation.  This
     * is package-private so only WMS owners can use it when rejecting a
     * duplicate registration.</p>
     */
    boolean sameAttachment(WindowSessionIdentity other) {
        return other != null
                && registry == other.registry
                && pid == other.pid
                && uid == other.uid
                && thread.equals(other.thread)
                && startSequence == other.startSequence;
    }
}
