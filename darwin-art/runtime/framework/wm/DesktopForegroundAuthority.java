package dev.darwinart.runtime.wm;

/** Process-instance identity and fresh host observations, never Android focus policy. */
final class DesktopForegroundAuthority {
    interface Provider {
        ProcessInstance capture(int pid);
        boolean isForeground(ProcessInstance process);
    }

    static final class ProcessInstance {
        final int pid;
        final long startSeconds;
        final long startMicroseconds;

        ProcessInstance(int pid, long seconds, long microseconds) {
            if (pid <= 0 || seconds <= 0 || microseconds < 0 || microseconds >= 1000000)
                throw new IllegalArgumentException("invalid host process instance");
            this.pid = pid;
            startSeconds = seconds;
            startMicroseconds = microseconds;
        }
    }

    static final Provider HOST = new Provider() {
        @Override public ProcessInstance capture(int pid) {
            long[] birth = nativeCaptureProcess(pid);
            if (birth == null || birth.length != 2)
                throw new IllegalStateException("host process identity unavailable");
            return new ProcessInstance(pid, birth[0], birth[1]);
        }

        @Override public boolean isForeground(ProcessInstance process) {
            if (process == null) throw new IllegalArgumentException("missing process identity");
            return nativeIsForeground(process.pid, process.startSeconds, process.startMicroseconds);
        }
    };

    private static native long[] nativeCaptureProcess(int pid);
    private static native boolean nativeIsForeground(int pid, long seconds, long microseconds);
    private DesktopForegroundAuthority() {}
}
