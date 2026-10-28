package dev.aim.server;
final class PackageLocalOwners {
    static Object publish(ISystemProcess host, android.content.Context context,
            com.android.server.pm.Installer installer) {
        SystemProcessHostTest.events.add("packageLocals");
        return new Object();
    }
}
