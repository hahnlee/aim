package dev.aim.server;
import android.app.ActivityManagerInternal;
import android.app.PendingIntentStats;
import android.util.StatsEvent;
import com.android.server.LocalServices;
import java.lang.reflect.Proxy;
import java.util.Arrays;

public final class ActivityStatsTest {
    static IActivityStatsSnapshot collector;
    static ActivityStatsPendingIntent[] pending;
    static CachedAppsWatermark watermark = new CachedAppsWatermark();
    static boolean reset;
    static void check(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) throws Exception {
        ActivityStatsProcess process = new ActivityStatsProcess();
        process.uid = 10123; process.pid = 42; process.processName = "a:remote";
        process.oomAdj = 900; process.procState = 16; process.hasForegroundServices = true;
        process.hostingComponentTypes = 256; process.historicalHostingComponentTypes = 384;
        IActivityStatsHost host = (IActivityStatsHost) Proxy.newProxyInstance(
                ActivityStatsTest.class.getClassLoader(), new Class<?>[]{IActivityStatsHost.class},
                (proxy, method, values) -> switch (method.getName()) {
                    case "getProcesses" -> new ActivityStatsProcess[]{process};
                    case "getPendingIntents" -> pending;
                    case "getCachedAppsWatermark" -> { reset = (boolean)values[0]; yield watermark; }
                    default -> throw new AssertionError(method);
                });
        ActivityStats.publish(snapshot -> { collector = snapshot; return host; });
        ActivityManagerInternal service = LocalServices.getService(ActivityManagerInternal.class);
        check(service.getClass().getSuperclass() == ActivityManagerInternal.class);
        var p = service.getMemoryStateForProcesses().get(0);
        check(p.uid == 10123 && p.pid == 42 && p.oomScore == 900 && p.hasForegroundServices
                && p.processName.equals("a:remote") && p.mHostingComponentTypes == 256
                && p.mHistoricalHostingComponentTypes == 384);
        pending = new ActivityStatsPendingIntent[3];
        for (int i = 0; i < 3; i++) {
            pending[i] = new ActivityStatsPendingIntent();
            pending[i].uid = i == 2 ? 10001 : 10123;
            pending[i].extrasBytes = 600;
        }
        var stats = service.getPendingIntentStats();
        check(stats.size() == 2 && stats.get(0).uid == 10001 && stats.get(0).sizeKb == 0);
        check(stats.get(1).uid == 10123 && stats.get(1).count == 2 && stats.get(1).sizeKb == 1);
        pending = new ActivityStatsPendingIntent[0];
        check(service.getPendingIntentStats().isEmpty());
        watermark.count = 1; watermark.uptimeSeconds = 2; watermark.binderProxies = 3;
        watermark.freeKb = 4; watermark.cachedKb = 5; watermark.zramKb = 6;
        watermark.kernelKb = 7; watermark.frozenApps = 8; watermark.longestFrozenSeconds = 9;
        watermark.shortestFrozenSeconds = 10; watermark.medianFrozenSeconds = 11;
        watermark.averageFrozenSeconds = 12;
        StatsEvent event = (StatsEvent)service.getCachedAppsHighWatermarkStats(10189, true);
        check(reset && event.atom == 10189 && event.values.equals(
                Arrays.asList(1,2,3,4,5,6,7,8,9,10,11,12)));
        service.getCachedAppsHighWatermarkStats(10189, false); check(!reset);
        CachedAppsWatermark snapshot = collector.snapshot(new int[]{10123});
        check(snapshot.binderProxies == 5 && snapshot.freeKb == 21 && snapshot.cachedKb == 22
                && snapshot.zramKb == 23 && snapshot.kernelKb == 24 && snapshot.frozenApps == 0);
        android.os.Binder.uid = 10123;
        try { collector.snapshot(new int[]{10123}); throw new AssertionError(); }
        catch (SecurityException expected) {}
        try { service.getPackageNameByPid(42); throw new AssertionError(); }
        catch (UnsupportedOperationException expected) {}
    }
}
