// Compile-only pinned image API.
package com.android.server.pm;
public abstract class ApexManager {
    public ApexManager() { throw new RuntimeException("stub"); }
    public static ApexManager getInstance() { throw new RuntimeException("stub"); }
    abstract android.apex.ApexInfo[] getAllApexInfos();
    public abstract java.util.List<ActiveApexInfo> getActiveApexInfos();
    public static class ActiveApexInfo {
        public final String apexModuleName;
        public final java.io.File apexDirectory, preInstalledApexPath, apexFile;
        public final boolean isFactory, activeApexChanged;
        public ActiveApexInfo(android.apex.ApexInfo info) { throw new RuntimeException("stub"); }
    }
}
