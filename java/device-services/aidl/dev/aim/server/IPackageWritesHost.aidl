package dev.aim.server;

/**
 * The native PackageManager's write model (docs/m4-packagemanager.md,
 * slice B), told of the original's install sessions by the bridge
 * (IBridge.watchPackageWrites) as their parameters stand, before the
 * original commits them: the input the model computes an install from.
 */
interface IPackageWritesHost {
    /**
     * Session `sessionId` as PackageInstaller.SessionInfo gives it now,
     * encoded as crates/aim-services/src/package/write/session.rs reads it.
     */
    void session(int sessionId, in byte[] info);

    /** Session `sessionId` ended; `success`: its install committed. */
    void finished(int sessionId, boolean success);
}
