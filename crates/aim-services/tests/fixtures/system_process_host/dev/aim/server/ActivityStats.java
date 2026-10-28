package dev.aim.server;
final class ActivityStats {
    static void publish(ISystemProcess host) { SystemProcessHostTest.events.add("activityStats"); }
}
