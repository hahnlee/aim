package com.android.server;
public class PackageWatchdog {
    public static int instances;
    public static PackageWatchdog getInstance(android.content.Context context) { instances++; return new PackageWatchdog(); }
    public void writeNow() { dev.aim.server.PackageShutdownJavaTest.step("watchdog"); }
}
