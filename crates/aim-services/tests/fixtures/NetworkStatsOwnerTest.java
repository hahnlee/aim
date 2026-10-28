package dev.aim.server.nearby;

import android.content.Context;
import android.net.*;
import android.net.connectivity.android.net.NetworkStatsAccess;
import android.net.connectivity.com.android.net.module.util.LocationPermissionChecker;
import android.os.Binder;

/** Production owner with host doubles for retained Android APIs; no guest boot. */
public final class NetworkStatsOwnerTest {
    private static int checks;
    private static void check(boolean value) {
        checks++;
        if (!value) throw new AssertionError("check " + checks);
    }
    private static void fails(Class<? extends Throwable> type, String message, Runnable call) {
        try { call.run(); } catch (Throwable e) {
            check(type.isInstance(e));
            if (message != null) check(e.getMessage().contains(message));
            return;
        }
        throw new AssertionError("expected " + type);
    }
    private static long operations(int uid, int tag) {
        return NetworkStats.operations.rows.stream().filter(e -> e.uid == uid && e.tag == tag)
                .mapToLong(e -> e.operations).sum();
    }
    public static void main(String[] args) {
        Context context = new Context();
        NetworkStatsInitializer.Owner owner = new NetworkStatsInitializer.Owner(context);
        owner.incrementOperationCount(10123, 7, 3);
        owner.incrementOperationCount(10123, 7, 5);
        owner.incrementOperationCount(10123, -1, 2);
        check(operations(10123, 7) == 8);
        check(operations(10123, -1) == 2);
        check(operations(10123, 0) == 10);
        check(context.checks == 0);
        fails(IllegalArgumentException.class, "only be incremented",
                () -> owner.incrementOperationCount(10123, 7, -1));
        fails(IllegalArgumentException.class, "specific tag",
                () -> owner.incrementOperationCount(10123, 0, 1));
        // The original checks permission before argument validation.
        fails(SecurityException.class, "UPDATE_DEVICE_STATS",
                () -> owner.incrementOperationCount(10124, 0, -1));
        check(operations(10123, 0) == 10 && operations(10124, 0) == 0);
        context.updateAllowed = true;
        owner.incrementOperationCount(10124, 9, 4);
        owner.incrementOperationCount(10124, 9, 0);
        check(operations(10124, 9) == 4 && operations(10124, 0) == 4);

        fails(NullPointerException.class, null, () -> owner.openSessionForUsageStats(0, null));
        fails(SecurityException.class, "package UID mismatch",
                () -> owner.openSessionForUsageStats(0, "another.pkg"));
        INetworkStatsSession session = (INetworkStatsSession) owner.openSessionForUsageStats(-1, "app.pkg");
        NetworkTemplate template = new NetworkTemplate();
        check(session.getRelevantUids().length == 0);
        check(session.getSummaryForAllUid(template, 12, 34, true).rows.isEmpty());
        check(session.getSummaryForAllUid(template, 12, 34, false).elapsed == 22);
        check(session.getTaggedSummaryForAllUid(template, 12, 34).rows.isEmpty());
        check(session.getHistoryForUid(template, 10123, -1, 0, -1).duration == 7200000L);
        check(session.getHistoryIntervalForUid(template, 10123, -1, 7, -1, 12, 34).duration == 7200000L);
        check(NetworkStatsCollection.caller == 10123 && NetworkStatsCollection.access == 0);
        fails(SecurityException.class, "inaccessible uid",
                () -> session.getHistoryForUid(template, 10124, -1, 0, -1));
        fails(SecurityException.class, "inaccessible uid",
                () -> session.getSummaryForNetwork(template, 12, 34));
        fails(SecurityException.class, "different uid",
                () -> session.getHistoryIntervalForUid(template, 10124, -1, 7, -1, 12, 34));
        // Access is captured on open, while the interval tag check uses the current caller.
        NetworkStatsAccess.level = 3;
        fails(SecurityException.class, "inaccessible uid",
                () -> session.getHistoryForUid(template, 10124, -1, 0, -1));
        INetworkStatsSession privileged = (INetworkStatsSession) owner.openSessionForUsageStats(0, "app.pkg");
        check(privileged.getDeviceSummaryForNetwork(template, 12, 34).rows.size() == 1);
        check(privileged.getSummaryForNetwork(template, 12, 34).rows.get(0).operations == 0);
        check(privileged.getHistoryForNetwork(template, -1).duration == 3600000L);
        check(privileged.getHistoryIntervalForNetwork(template, -1, 12, 34).duration == 3600000L);
        check(privileged.getHistoryForUid(template, 10124, -1, 7, -1).duration == 7200000L);
        fails(SecurityException.class, "different uid",
                () -> privileged.getHistoryIntervalForUid(template, 10124, -1, 7, -1, 12, 34));
        template.keys = java.util.Set.of("wifi-key");
        fails(SecurityException.class, "fine location",
                () -> session.getSummaryForAllUid(template, 12, 34, false));
        LocationPermissionChecker.allowed = true;
        check(session.getSummaryForAllUid(template, 12, 34, true).rows.isEmpty());
        LocationPermissionChecker.allowed = false;
        template.rule = NetworkTemplate.MATCH_TEST;
        check(session.getSummaryForAllUid(template, 12, 34, true).rows.isEmpty());
        // close drops loaded collection references; the original allows querying again.
        session.close();
        session.close();
        check(session.getRelevantUids().length == 0);
        check(session.getSummaryForAllUid(template, 12, 34, true).rows.isEmpty());
        privileged.close();
        check(operations(10123, 0) == 10);
        System.out.println("NetworkStats owner: " + checks + " checks passed");
    }
}
