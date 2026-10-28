package dev.aim.server;

/** StatsCompanionService's cold UID-map/receiver lifecycle, system UID only. */
interface IStatsCompanionUidMap {
    /** Receivers and initial UID map after statsCompanionReady; unregister on statsd death. */
    void start(IBinder statsd);
}
