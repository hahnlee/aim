package com.android.server.pm;

import com.android.internal.pm.parsing.PackageParser2;
import com.android.internal.pm.parsing.pkg.PackageImpl;

public final class UpdateOwnershipResourceOracle {
    public static void main(String[] args) throws Exception {
        var config = new com.android.server.SystemConfig(false);
        config.readPermissions(android.util.Xml.newPullParser(), new java.io.File(args[0]), 0);
        System.out.print("POLICY");
        for (String name : new String[] {"app", "missing", " ", "nested"}) {
            String installer = config.getSystemAppUpdateOwnerPackageName(name);
            System.out.print(" ");
            if (installer == null) System.out.print("null");
            else for (byte b : installer.getBytes(java.nio.charset.StandardCharsets.UTF_8))
                System.out.printf("%02x", b & 255);
        }
        System.out.println();
        var thread = new android.os.HandlerThread("AIM disposable resource oracle");
        thread.start();
        var handler = thread.getThreadHandler();
        var gate = new java.util.concurrent.CountDownLatch(1);
        var completed = new java.util.concurrent.CountDownLatch(args.length - 1);
        var failure = new java.util.concurrent.atomic.AtomicReference<Throwable>();
        if (!handler.post(() -> {
            try { gate.await(); }
            catch (InterruptedException error) { failure.set(error); }
        })) throw new IllegalStateException("handler gate post rejected");
        try (var parser = new PackageParser2(null, null, null, new PackageParser2.Callback() {
            public boolean hasFeature(String feature) { return false; }
            public java.util.Set<String> getHiddenApiWhitelistedApps() { return java.util.Set.of(); }
            public java.util.Set<String> getInstallConstraintsAllowlist() { return java.util.Set.of(); }
            public boolean isChangeEnabled(long changeId, android.content.pm.ApplicationInfo info) { return false; }
        })) {
            var owner = new UpdateOwnershipHelper();
            for (int i = 1; i < args.length; i++) {
                String path = args[i];
                var file = new java.io.File(path);
                if (file.isDirectory()) {
                    var configuration = new android.content.res.Configuration();
                    configuration.seq = 1;
                    configuration.setLocale(java.util.Locale.ENGLISH);
                    if (!android.app.ResourcesManager.getInstance().applyConfigurationToResources(configuration, null))
                        throw new IllegalStateException("resource locale change was rejected");
                }
                var pkg = (PackageImpl) parser.parsePackage(file, 0, false);
                var appInfo = com.android.server.pm.parsing.pkg.AndroidPackageUtils.generateAppInfoWithoutState(pkg);
                if (appInfo.resourceDirs != null || appInfo.overlayPaths != null || appInfo.sharedLibraryFiles != null)
                    throw new IllegalStateException("without-state resources contain app-state assets");
                if (file.isDirectory() && (appInfo.splitSourceDirs == null || appInfo.splitSourceDirs.length != 2))
                    throw new IllegalStateException("without-state resources lost a split");
                var setting = new PackageSetting(pkg.getPackageName(), null, file, 1, 0,
                        java.util.UUID.randomUUID()).setPkg(pkg);
                if (!handler.post(() -> {
                    try {
                        var contents = owner.readUpdateOwnerDenyList(setting);
                        if (contents == null) throw new IllegalStateException("original resource read failed: " + path);
                        System.out.print(file.getName());
                        for (String name : contents) {
                            System.out.print(" ");
                            for (byte b : name.getBytes(java.nio.charset.StandardCharsets.UTF_8))
                                System.out.printf("%02x", b & 255);
                        }
                        System.out.println();
                    } catch (Throwable error) { failure.compareAndSet(null, error); }
                    finally { completed.countDown(); }
                })) throw new IllegalStateException("handler resource post rejected");
            }
            gate.countDown();
            if (!completed.await(30, java.util.concurrent.TimeUnit.SECONDS))
                throw new IllegalStateException("original posted resource reads did not complete");
            if (failure.get() != null) throw new IllegalStateException("original posted resource read failed", failure.get());
        } finally {
            gate.countDown();
            thread.quitSafely();
            thread.join(5000);
            if (thread.isAlive()) throw new IllegalStateException("resource oracle thread remains alive");
        }
    }
}
