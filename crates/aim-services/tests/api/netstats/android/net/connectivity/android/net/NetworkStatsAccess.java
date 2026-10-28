// Host JVM test double; production links the retained image classes.
package android.net.connectivity.android.net;
public class NetworkStatsAccess {
 public @interface Level {}
 public static int level;
 public static int checkAccessLevel(android.content.Context context,int pid,int uid,String pkg){
  if(pid!=android.os.Binder.pid||uid!=android.os.Binder.uid)throw new AssertionError("caller identity");
  return level;
 }
}
