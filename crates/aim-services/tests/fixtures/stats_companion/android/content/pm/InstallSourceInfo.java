// Host-only Android boundary double; production policy is compiled unchanged.
package android.content.pm;
public class InstallSourceInfo {
    public String initiating,installing;
    public String getInitiatingPackageName() { return initiating; }
    public String getInstallingPackageName() { return installing; }
}
