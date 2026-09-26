package dev.darwinart.runtime.system;
// Test-only substitute: records that the host command relay was started.
public final class HostCommandService {
    public static int started;
    public static void start(ServiceDirectory directory) { started++; }
}
