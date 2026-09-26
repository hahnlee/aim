package dev.darwinart.runtime.system;
// Test-only substitute for the AOSP bootstrap services sequence.
public final class SystemServerBootstrap {
    public static int started;
    public static void startBootstrapServices(android.content.Context context) { started++; }
}
