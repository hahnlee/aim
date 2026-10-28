// Host JVM test double; production links the retained image classes.
package android.net;
import java.util.*;
public final class NetworkStats {
 public static final String IFACE_ALL=null;
 public static final int UID_ALL=-1,SET_ALL=-1,SET_DEFAULT=0,TAG_NONE=0,METERED_ALL=-1,ROAMING_ALL=-1,DEFAULT_NETWORK_ALL=-1;
 public static NetworkStats operations;
 public final long elapsed;
 public final List<Entry> rows=new ArrayList<>();
 public NetworkStats(long elapsed,int size){this.elapsed=elapsed;if(size==10)operations=this;}
 public NetworkStats combineValues(String iface,int uid,int set,int tag,long rb,long rp,long tb,long tp,long ops){
  if(iface!=null||set!=0||rb!=0||rp!=0||tb!=0||tp!=0)throw new AssertionError("unrecorded fields");
  for(Entry e:rows)if(e.uid==uid&&e.tag==tag){e.operations+=ops;return this;}
  rows.add(new Entry(iface,uid,set,tag,0,0,0,rb,rp,tb,tp,ops));return this;
 }
 public void combineAllValues(NetworkStats other){rows.addAll(other.rows);}
 public NetworkStats insertEntry(Entry e){rows.add(e);return this;}
 public static class Entry {
  public int uid,tag; public long operations;
  public Entry(String iface,int uid,int set,int tag,int metered,int roaming,int def,long rb,long rp,long tb,long tp,long ops){this.uid=uid;this.tag=tag;operations=ops;}
 }
}
