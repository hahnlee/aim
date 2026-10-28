package dev.aim.server;
import dev.aim.server.CachedAppsWatermark;
/** Cold Java snapshot at the process owner's new cached-app high watermark. */
interface IActivityStatsSnapshot {
    CachedAppsWatermark snapshot(in int[] liveUids);
}
