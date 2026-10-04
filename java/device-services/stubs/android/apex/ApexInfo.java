// Compile-only pinned image API.
package android.apex;
public class ApexInfo {
    public String moduleName, modulePath, preinstalledModulePath;
    public long versionCode;
    public boolean isFactory, isActive, activeApexChanged;
    public ApexInfo() { throw new RuntimeException("stub"); }
}
