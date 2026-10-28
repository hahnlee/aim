package dev.aim.server;
final class PackageBootstrapBridge extends android.os.Binder {
    void installPackageShutdownOwner(Object owner) {
        if (owner == null) throw new AssertionError();
        SystemProcessHostTest.events.add("packageShutdown");
    }
    PackageBootstrapBridge(Object domains, com.android.server.pm.Installer installer) {
        if (domains != null || installer == null) throw new AssertionError();
    }
}
