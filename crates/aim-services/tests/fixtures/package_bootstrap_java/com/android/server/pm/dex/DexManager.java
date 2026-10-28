package com.android.server.pm.dex;
public class DexManager {
    public static DexManager current;
    public java.util.Map<Integer, java.util.List<android.content.pm.PackageInfo>> installed;
    public DexManager(android.content.Context context, com.android.server.pm.PackageDexOptimizer optimizer,
            DynamicCodeLogger dynamic) { current = this; }
    public void load(java.util.Map<Integer, java.util.List<android.content.pm.PackageInfo>> packages) { installed = packages; }
    public void writePackageDexUsageNow() { dev.aim.server.PackageShutdownJavaTest.step("dex"); }
}
