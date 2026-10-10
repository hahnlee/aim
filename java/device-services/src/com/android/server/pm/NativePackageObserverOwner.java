package com.android.server.pm;

import android.content.pm.PackageManagerInternal.PackageListObserver;
import android.os.Binder;
import android.os.Process;
import android.util.ArraySet;
import dev.aim.server.IPackageObserverEventsBridge;
import dev.aim.server.NativePackageManagerInternal;
import dev.aim.server.PackageSnapshots;

/** One native PMS epoch's actual PackageList observers, with original identity semantics. */
public final class NativePackageObserverOwner extends IPackageObserverEventsBridge.Stub
        implements NativePackageManagerInternal.ObserverOwner, AutoCloseable {
    private final PackageSnapshots.Store packages;
    private final PackageObserverHelper helper = new PackageObserverHelper();
    private final ArraySet<PackageListObserver> registered = new ArraySet<>();
    private boolean closed;
    public NativePackageObserverOwner(PackageSnapshots.Store packages) {
        this.packages = java.util.Objects.requireNonNull(packages);
    }
    private synchronized void requireOpen() {
        if (closed) throw new IllegalStateException("native package observer epoch closed");
    }
    private static void enforceTransport() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID)
            throw new SecurityException("native package observer transport requires system UID");
    }
    public PackageList getPackageList(PackageListObserver observer) {
        requireOpen();
        java.util.ArrayList<String> names = new java.util.ArrayList<>();
        try (var scope = packages.computer()) {
            scope.forEachPackage(pkg -> names.add(pkg.getPackageName()));
        }
        // PackageList keeps the exact wrapped Java observer and removes itself
        // through the native LocalServices facade on close, as the original does.
        PackageList result = new PackageList(names, observer);
        if (observer != null) addPackageListObserver(result);
        return result;
    }
    @Override public synchronized void addPackageListObserver(PackageListObserver observer) {
        requireOpen();
        helper.addObserver(observer);
        registered.add(observer);
    }
    @Override public synchronized void removePackageListObserver(PackageListObserver observer) {
        // Removal on a closed epoch is still a valid AutoCloseable cleanup.
        helper.removeObserver(observer);
        registered.remove(observer);
    }
    @Override public void packageAdded(String name, int uid) {
        enforceTransport(); requireOpen(); helper.notifyAdded(name, uid);
    }
    @Override public void packageChanged(String name, int uid) {
        enforceTransport(); requireOpen(); helper.notifyChanged(name, uid);
    }
    @Override public void packageRemoved(String name, int uid) {
        enforceTransport(); requireOpen(); helper.notifyRemoved(name, uid);
        android.content.pm.UserPackage.removeFromCache(android.os.UserHandle.getUserId(uid),name);
    }
    @Override public void revoke() { enforceTransport(); close(); }
    @Override public synchronized void close() {
        if (closed) return;
        closed = true;
        for (int i = 0; i < registered.size(); i++) helper.removeObserver(registered.valueAt(i));
        registered.clear();
    }
}
