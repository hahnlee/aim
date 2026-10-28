package dev.aim.server;

/** StatsCompanionService's cold lifecycle and diagnostics, system UID only. */
interface IStatsCompanionUidMap {
    /** Receivers and initial UID map after statsCompanionReady; unregister on statsd death. */
    void start(IBinder statsd);
    /** Original dump text; the native endpoint checks the caller's DUMP permission. */
    String dumpStatsCompanion();
}
