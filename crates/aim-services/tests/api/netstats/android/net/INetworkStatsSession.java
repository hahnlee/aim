// Host JVM test double; production links the retained image classes.
// Compile-only image API, checked by device-services linkage.
package android.net;
public interface INetworkStatsSession extends android.os.IInterface {
 NetworkStats getDeviceSummaryForNetwork(NetworkTemplate template, long start, long end);
 NetworkStats getSummaryForNetwork(NetworkTemplate template, long start, long end);
 NetworkStatsHistory getHistoryForNetwork(NetworkTemplate template, int fields);
 NetworkStatsHistory getHistoryIntervalForNetwork(NetworkTemplate template, int fields, long start, long end);
 NetworkStats getSummaryForAllUid(NetworkTemplate template, long start, long end, boolean includeTags);
 NetworkStats getTaggedSummaryForAllUid(NetworkTemplate template, long start, long end);
 NetworkStatsHistory getHistoryForUid(NetworkTemplate template, int uid, int set, int tag, int fields);
 NetworkStatsHistory getHistoryIntervalForUid(NetworkTemplate template, int uid, int set, int tag, int fields, long start, long end);
 int[] getRelevantUids();
 void close();
 abstract class Stub extends android.os.Binder implements INetworkStatsSession { public android.os.IBinder asBinder() { return this; } }
}

