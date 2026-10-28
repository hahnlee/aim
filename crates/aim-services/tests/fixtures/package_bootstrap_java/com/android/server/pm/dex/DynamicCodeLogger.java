package com.android.server.pm.dex;
public class DynamicCodeLogger {
    public DynamicCodeLogger(com.android.server.pm.Installer installer) {}
    public java.util.Map<Integer, java.util.List<android.content.pm.PackageInfo>> installed;
    public void load(java.util.Map<Integer, java.util.List<android.content.pm.PackageInfo>> packages) { installed = packages; }
    public void writeNow() { dev.aim.server.PackageShutdownJavaTest.step("dynamic"); }
}
