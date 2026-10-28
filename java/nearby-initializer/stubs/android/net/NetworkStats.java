// Compile-only image API, checked by device-services linkage.
package android.net;
public final class NetworkStats {
 public static final String IFACE_ALL = null;
 public static final int UID_ALL=-1, SET_ALL=-1, SET_DEFAULT=0, TAG_NONE=0,
  METERED_ALL=-1, ROAMING_ALL=-1, DEFAULT_NETWORK_ALL=-1;
 public NetworkStats(long elapsedRealtime, int initialSize) { throw new RuntimeException("stub"); }
 public NetworkStats combineValues(String iface,int uid,int set,int tag,long rb,long rp,long tb,long tp,long ops) { throw new RuntimeException("stub"); }
 public void combineAllValues(NetworkStats other) { throw new RuntimeException("stub"); }
 public NetworkStats insertEntry(Entry e) { throw new RuntimeException("stub"); }
 public static class Entry {
  public Entry(String iface,int uid,int set,int tag,int metered,int roaming,int def,long rb,long rp,long tb,long tp,long ops) { throw new RuntimeException("stub"); }
 }
}
