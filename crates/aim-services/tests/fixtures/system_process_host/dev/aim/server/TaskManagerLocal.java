package dev.aim.server;
final class TaskManagerLocal {
    static void publish(ISystemProcess host) { SystemProcessHostTest.events.add("taskManagerLocal"); }
}
