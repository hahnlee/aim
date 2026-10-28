package dev.aim.server;
final class PackageBootstrapBridge extends android.os.Binder {
    PackageBootstrapBridge(Object domains, com.android.server.pm.Installer installer) {
        if (domains != null || installer == null) throw new AssertionError();
    }
}
