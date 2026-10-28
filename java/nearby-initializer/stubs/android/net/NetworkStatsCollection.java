// Compile-only image API, checked by device-services linkage.
package android.net;
import android.telephony.SubscriptionPlan;
public class NetworkStatsCollection {
 public NetworkStatsCollection(long bucketDuration) { throw new RuntimeException("stub"); }
 public int[] getRelevantUids(int access) { throw new RuntimeException("stub"); }
 public NetworkStats getSummary(NetworkTemplate template,long start,long end,int access,int uid) { throw new RuntimeException("stub"); }
 public NetworkStatsHistory getHistory(NetworkTemplate template,SubscriptionPlan plan,int uid,int set,int tag,int fields,long start,long end,int access,int caller) { throw new RuntimeException("stub"); }
}
