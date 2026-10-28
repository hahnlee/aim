package dev.aim.server;
final class PreBootNotifications {
    static void publish(android.content.Context context) {
        if (context == null) throw new AssertionError("system context missing");
    }
}
