package dev.aim.server;

import android.app.ActivityManagerInternal;
import android.app.PendingIntentStats;
import android.app.ProcessMemoryState;
import android.os.RemoteException;
import android.util.StatsEvent;
import com.android.internal.os.BinderInternal;
import com.android.internal.util.MemInfoReader;
import com.android.server.LocalServices;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.TreeMap;

/**
 * Aggregation and watermark fields follow AOSP android-16.0.0_r1
 * PendingIntentController/AppProfiler (Apache-2.0).
 * Cold local pull adapter over the app host's process and PendingIntent owners (#2274).
 */
final class ActivityStats extends ActivityManagerInternal {
    private final IActivityStatsHost host;

    ActivityStats(IActivityStatsHost host) { this.host = host; }

    static void publish(ISystemProcess system) throws RemoteException {
        IActivityStatsSnapshot snapshot = new IActivityStatsSnapshot.Stub() {
            @Override public CachedAppsWatermark snapshot(int[] liveUids) {
                Bridge.enforceSystemUid();
                CachedAppsWatermark data = new CachedAppsWatermark();
                var counts = BinderInternal.nGetBinderProxyPerUidCounts();
                if (counts != null) {
                    for (int i = 0; i < counts.size(); i++) {
                        if (Arrays.binarySearch(liveUids, counts.keyAt(i)) >= 0) {
                            data.binderProxies += counts.valueAt(i);
                        }
                    }
                }
                MemInfoReader memory = new MemInfoReader();
                memory.readMemInfo();
                data.freeKb = (int) memory.getFreeSizeKb();
                data.cachedKb = (int) memory.getCachedSizeKb();
                data.zramKb = (int) memory.getZramTotalSizeKb();
                data.kernelKb = (int) memory.getKernelUsedSizeKb();
                // The native process owner never freezes an app.
                return data;
            }
        };
        LocalServices.addService(ActivityManagerInternal.class,
                new ActivityStats(system.getActivityStatsHost(snapshot)));
    }

    @Override public List<ProcessMemoryState> getMemoryStateForProcesses() {
        try {
            List<ProcessMemoryState> result = new ArrayList<>();
            for (ActivityStatsProcess p : host.getProcesses()) {
                result.add(new ProcessMemoryState(p.uid, p.pid, p.processName, p.oomAdj,
                        p.hasForegroundServices, p.hostingComponentTypes, p.historicalHostingComponentTypes));
            }
            // The original pullers obtain RSS/swap from the syscall layer's procfs.
            return result;
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }

    @Override public List<PendingIntentStats> getPendingIntentStats() {
        try {
            TreeMap<Integer, int[]> totals = new TreeMap<>();
            for (ActivityStatsPendingIntent p : host.getPendingIntents()) {
                int[] total = totals.computeIfAbsent(p.uid, uid -> new int[2]);
                total[0]++;
                total[1] += p.extrasBytes;
            }
            List<PendingIntentStats> result = new ArrayList<>();
            totals.forEach((uid, total) -> result.add(
                    new PendingIntentStats(uid, total[0], total[1] / 1024)));
            return result;
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }

    @Override public Object getCachedAppsHighWatermarkStats(int atomTag, boolean resetAfterPull) {
        try {
            CachedAppsWatermark d = host.getCachedAppsWatermark(resetAfterPull);
            return StatsEvent.newBuilder().setAtomId(atomTag)
                    .writeInt(d.count).writeInt(d.uptimeSeconds).writeInt(d.binderProxies)
                    .writeInt(d.freeKb).writeInt(d.cachedKb).writeInt(d.zramKb)
                    .writeInt(d.kernelKb).writeInt(d.frozenApps).writeInt(d.longestFrozenSeconds)
                    .writeInt(d.shortestFrozenSeconds).writeInt(d.medianFrozenSeconds)
                    .writeInt(d.averageFrozenSeconds).build();
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }

    private static UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("ActivityManagerInternal outside stats pulls (#2274)");
    }
    @Override public void killApplicationSync(String name, int appId, int user, String reason, int exitReason) { throw unsupported(); }
    @Override public String getPackageNameByPid(int pid) { throw unsupported(); }
    @Override public long getBootTimeTempAllowListDuration() { throw unsupported(); }
    @Override public int broadcastIntentWithCallback(android.content.Intent intent,
            android.content.IIntentReceiver receiver, String[] permissions, int user, int[] appIds,
            java.util.function.BiFunction<Integer, android.os.Bundle, android.os.Bundle> filter,
            android.os.Bundle options) { throw unsupported(); }
}
