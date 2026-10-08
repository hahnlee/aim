package com.android.server.pm;

import android.os.RemoteException;
import dev.aim.server.IPackageUserOperations;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.atomic.AtomicReference;

/** Original UM construction before package scanning and permission creation. */
public final class NativeEarlyPackageUserOwner implements NativeUserManagerBridge.Owner {
    private final IPackageUserOperations early;
    private final AtomicReference<NativePackageUserOwner> production = new AtomicReference<>();
    public NativeEarlyPackageUserOwner(IPackageUserOperations early) {
        this.early = Objects.requireNonNull(early);
    }
    /** Root completes the real permission/native snapshot constructor first. */
    public void attachAfterScan(NativePackageUserOwner owner) {
        if (!production.compareAndSet(null, Objects.requireNonNull(owner))) {
            throw new IllegalStateException("production UM package owner already attached");
        }
    }
    private NativePackageUserOwner requireProduction() {
        NativePackageUserOwner owner = production.get();
        if (owner == null) throw new IllegalStateException("native package scan and permission owners are not constructed");
        return owner;
    }
    @Override public boolean hasSystemFeature(String name, int version) {
        NativePackageUserOwner owner = production.get();
        if (owner != null) return owner.hasSystemFeature(name, version);
        try { return early.hasSystemFeature(name, version); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public boolean isDeviceUpgrading() {
        NativePackageUserOwner owner = production.get();
        if (owner != null) return owner.isDeviceUpgrading();
        try { return early.isDeviceUpgrading(); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void createNewUser(int user, Set<String> installable, String[] disallowed) {
        requireProduction().createNewUser(user, installable, disallowed);
    }
    @Override public void onNewUserCreated(int user, boolean converted) {
        requireProduction().onNewUserCreated(user, converted);
    }
    @Override public void cleanUpUser(UserManagerService users, int user) {
        requireProduction().cleanUpUser(users, user);
    }
    @Override public Computer snapshotComputer() { return requireProduction().snapshotComputer(); }
    @Override public void addCrossProfileIntentFilter(Computer snapshot, WatchedIntentFilter filter,
            String owner, int source, int target, int flags) {
        requireProduction().addCrossProfileIntentFilter(snapshot, filter, owner, source, target, flags);
    }
}
