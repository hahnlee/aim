// Host JVM test double; production links the retained image classes.
package android.net;
import android.telephony.SubscriptionPlan;
public class NetworkStatsCollection {
 private final long duration;
 public static int caller,access,uid,tag;
 public NetworkStatsCollection(long duration){this.duration=duration;}
 public int[] getRelevantUids(int level){return new int[0];}
 public NetworkStats getSummary(NetworkTemplate template,long start,long end,int level,int callingUid){caller=callingUid;access=level;return new NetworkStats(end-start,0);}
 public NetworkStatsHistory getHistory(NetworkTemplate template,SubscriptionPlan plan,int uid,int set,int tag,int fields,long start,long end,int level,int callingUid){
  caller=callingUid;access=level;NetworkStatsCollection.uid=uid;NetworkStatsCollection.tag=tag;
  if(level==0&&uid!=callingUid)throw new SecurityException("inaccessible uid");
  return new NetworkStatsHistory(duration);
 }
}
