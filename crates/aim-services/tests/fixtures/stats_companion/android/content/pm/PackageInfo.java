// Host-only Android boundary double; production policy is compiled unchanged.
package android.content.pm;
public class PackageInfo {
    public String packageName,versionName;
    public long version;
    public boolean isApex;
    public ApplicationInfo applicationInfo;
    public SigningInfo signingInfo;
    public long getLongVersionCode() { return version; }
}
