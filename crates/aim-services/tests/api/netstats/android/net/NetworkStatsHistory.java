// Host JVM test double; production links the retained image classes.
package android.net;
public class NetworkStatsHistory {
 public static final int FIELD_ALL=-1;
 public final long duration;
 public NetworkStatsHistory(long duration){this.duration=duration;}
 public Entry getValues(long start,long end,long now,Entry recycle){return new Entry();}
 public static class Entry {public long rxBytes,rxPackets,txBytes,txPackets,operations;}
}
