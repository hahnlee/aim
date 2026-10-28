// Host-only Android boundary double; production policy is compiled unchanged.
package android.content;
public class IntentFilter {
    public final java.util.Set<String> actions=new java.util.HashSet<>();
    public String scheme;
    public IntentFilter(String action) { actions.add(action); }
    public void addAction(String action) { actions.add(action); }
    public void addDataScheme(String scheme) { this.scheme=scheme; }
}
