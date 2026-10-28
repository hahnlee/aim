package dev.aim.server;
final class DeviceServices {
    static IForegroundServicePolicy foregroundServicePolicy(android.content.Context context) {
        SystemProcessHostTest.events.add("foreground");
        return SystemProcessHostTest.foregroundPolicy;
    }
}
