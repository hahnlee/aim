package dev.darwinart.runtime.am;

/**
 * Narrow process-launch resource port used by Android's service demand owner.
 * Every operation may reenter Android and must run outside the AMS monitor.
 * The token is transport-owned; Android policy never receives a native handle.
 */
interface ProcessLaunchTransport {
    interface PreparedLaunch {
        int pid();
        long startSequence();
    }

    enum ActivationResult {
        ACTIVE,
        /** A genuine registry attachment owns the process; do not cancel it. */
        HANDOFF,
        /** Exact unattached retirement won. This is not child-reap proof. */
        CANCELLED
    }

    PreparedLaunch prepare(String packageName, String processName, int uid,
            boolean isolated, long startSequence);
    ActivationResult activate(PreparedLaunch launch);
    void retireUnattached(PreparedLaunch launch);
    /** Resource cleanup for a captured, genuinely terminal Android process incarnation. */
    void onProcessGone(ApplicationProcessRegistry.AttachedApplication gone);
}
