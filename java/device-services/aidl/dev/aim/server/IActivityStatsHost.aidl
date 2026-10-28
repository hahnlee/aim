package dev.aim.server;
import dev.aim.server.ActivityStatsProcess;
import dev.aim.server.ActivityStatsPendingIntent;
import dev.aim.server.CachedAppsWatermark;
/** System-UID-only snapshots from the live activity and PendingIntent owners. */
interface IActivityStatsHost {
    ActivityStatsProcess[] getProcesses();
    ActivityStatsPendingIntent[] getPendingIntents();
    CachedAppsWatermark getCachedAppsWatermark(boolean resetAfterPull);
}
