// Host JVM test double; production links the retained image classes.
package android.net;
public final class NetworkTemplate {
 public static final int MATCH_TEST=11;
 public int rule=1;
 public java.util.Set<String> keys=java.util.Set.of();
 public java.util.Set<String> getWifiNetworkKeys(){return keys;}
 public int getMatchRule(){return rule;}
}
