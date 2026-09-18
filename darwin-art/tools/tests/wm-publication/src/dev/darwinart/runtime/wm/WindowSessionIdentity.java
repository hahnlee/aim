package dev.darwinart.runtime.wm;

/** Minimal identity seam retaining the complete attachment incarnation. */
final class WindowSessionIdentity {
    private final int pid;
    private final int uid;
    private final Object attachment;

    WindowSessionIdentity(int pid, int uid, Object attachment) {
        this.pid = pid;
        this.uid = uid;
        this.attachment = attachment;
    }

    int pid() { return pid; }
    int uid() { return uid; }
    void requireCaller(int callerPid, int callerUid) {
        if (pid != callerPid || uid != callerUid) throw new SecurityException("fixture caller mismatch");
    }

    boolean sameAttachment(WindowSessionIdentity other) {
        return other != null && pid == other.pid && uid == other.uid
                && attachment == other.attachment;
    }
}
