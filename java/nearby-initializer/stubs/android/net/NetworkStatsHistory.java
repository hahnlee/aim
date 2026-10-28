// Compile-only image API, checked by device-services linkage.
package android.net;
public class NetworkStatsHistory {
 public NetworkStatsHistory(long duration) { throw new RuntimeException("stub"); }
 public static final int FIELD_ALL=-1;
 public Entry getValues(long start,long end,long now,Entry recycle) { throw new RuntimeException("stub"); }
 public static class Entry { public long rxBytes,rxPackets,txBytes,txPackets,operations; }
}
