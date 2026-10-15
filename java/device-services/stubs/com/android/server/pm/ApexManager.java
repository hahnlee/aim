// Compile-only pinned image API.
package com.android.server.pm;
public abstract class ApexManager {
    public abstract java.util.List<String> getApksInApex(String packageName);
    public abstract void registerApkInApex(com.android.server.pm.pkg.AndroidPackage pkg);
    android.apex.ApexInfoList submitStagedSession(android.apex.ApexSessionParams params) throws PackageManagerException { throw new RuntimeException("stub"); }
    void markStagedSessionReady(int id) throws PackageManagerException { throw new RuntimeException("stub"); }
    boolean abortStagedSession(int id) { throw new RuntimeException("stub"); }
    void markStagedSessionSuccessful(int id) { throw new RuntimeException("stub"); }
    public ApexManager() { throw new RuntimeException("stub"); }
    public static ApexManager getInstance() { throw new RuntimeException("stub"); }
    abstract android.apex.ApexInfo[] getAllApexInfos();
    abstract android.apex.ApexInfo[] getStagedApexInfos(android.apex.ApexSessionParams params);
    abstract void notifyScanResult(java.util.List<ScanResult> results);
    public abstract String getApexModuleNameForPackageName(String packageName);
    public abstract String getActivePackageNameForApexModuleName(String moduleName);
    static class ScanResult {
        public final android.apex.ApexInfo apexInfo;
        public final com.android.server.pm.pkg.AndroidPackage pkg;
        public final String packageName;
        ScanResult(android.apex.ApexInfo info, com.android.server.pm.pkg.AndroidPackage pkg, String name) { throw new RuntimeException("stub"); }
    }
    protected static class ApexManagerImpl extends ApexManager {
        public java.util.List<String> getApksInApex(String packageName) { throw new RuntimeException("stub"); }
        public void registerApkInApex(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
        public ApexManagerImpl() { throw new RuntimeException("stub"); }
        android.apex.ApexInfo[] getStagedApexInfos(android.apex.ApexSessionParams params) { throw new RuntimeException("stub"); }
        android.apex.ApexInfo[] getAllApexInfos() { throw new RuntimeException("stub"); }
        void notifyScanResult(java.util.List<ScanResult> results) { throw new RuntimeException("stub"); }
        public java.util.List<ActiveApexInfo> getActiveApexInfos() { throw new RuntimeException("stub"); }
        public String getApexModuleNameForPackageName(String name) { throw new RuntimeException("stub"); }
        public String getActivePackageNameForApexModuleName(String name) { throw new RuntimeException("stub"); }
    }
    public abstract java.util.List<ActiveApexInfo> getActiveApexInfos();
    public static class ActiveApexInfo {
        public final String apexModuleName;
        public final java.io.File apexDirectory, preInstalledApexPath, apexFile;
        public final boolean isFactory, activeApexChanged;
        public ActiveApexInfo(android.apex.ApexInfo info) { throw new RuntimeException("stub"); }
    }
 boolean isApexSupported(){throw new RuntimeException("stub");}
 boolean uninstallApex(String path){throw new RuntimeException("stub");}
 android.apex.ApexSessionInfo getStagedSessionInfo(int id){throw new RuntimeException("stub");}
}
