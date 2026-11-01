package dev.aim.server;
final class NativeJobScheduler {
    static void publish(ISystemProcess owner) { SystemProcessHostTest.events.add("jobLocal"); }
}
