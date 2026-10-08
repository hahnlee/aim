package com.android.server.pm;

import android.content.Context;
import android.os.Parcel;
import android.os.RemoteException;
import dev.aim.server.IPackageUserOperations;
import dev.aim.server.PackagePermissionLifecycle;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import java.util.Objects;
import java.util.Set;
import java.util.function.Supplier;

/** Concrete owner of every original UM redirect, without an original PMS. */
public final class NativePackageUserOwner implements NativeUserManagerBridge.Owner,
        PackagePermissionLifecycle.StateOwner {
    private final IPackageUserOperations host;
    private final Context context;
    private final Installer installer;
    private final PackageManagerTracedLock installLock;
    private final Object packagesLock;
    private final Supplier<Computer> computers;
    private final PackagePermissionLifecycle permissions;

    public NativePackageUserOwner(Context context, Installer installer,
            PackageManagerTracedLock installLock, Object packagesLock,
            IPackageUserOperations host, Supplier<Computer> computers,
            PermissionManagerServiceInternal permissionOwner,
            LegacyPermissionManagerInternal legacyPermissions, String buildFingerprint) {
        this.context = Objects.requireNonNull(context);
        this.installer = Objects.requireNonNull(installer);
        this.installLock = Objects.requireNonNull(installLock);
        this.packagesLock = Objects.requireNonNull(packagesLock);
        this.host = Objects.requireNonNull(host);
        this.computers = Objects.requireNonNull(computers);
        permissions = new PackagePermissionLifecycle(permissionOwner, legacyPermissions, this,
                Objects.requireNonNull(buildFingerprint));
    }
    @SuppressWarnings("try")
    @Override public void createNewUser(int user, Set<String> installable, String[] disallowed) {
        try (PackageManagerTracedLock ignored = installLock.acquireLock()) {
            var users = Objects.requireNonNull(com.android.server.LocalServices.getService(UserManagerInternal.class));
            var info = Objects.requireNonNull(users.getUserInfo(user), "new original user unavailable");
            Parcel userParcel = Parcel.obtain();
            byte[] userRecord;
            try {
                userParcel.writeInt(1);
                userParcel.writeInt(info.id);
                userParcel.writeInt(info.flags);
                userParcel.writeInt(info.profileGroupId);
                userParcel.writeBoolean(users.isUserUnlockingOrUnlocked(user));
                userParcel.writeBoolean(info.preCreated);
                userParcel.writeBoolean(users.hasUserRestriction(android.os.UserManager.DISALLOW_DEBUGGING_FEATURES, user));
                userRecord = userParcel.marshall();
            } finally { userParcel.recycle(); }
            byte[] bytes = host.createUserState(user,
                    installable == null ? null : installable.toArray(new String[0]), disallowed,
                    System.currentTimeMillis(), NativeUserAppData.stopSystemPackagesByDefault(context), userRecord);
            if (bytes == null) throw new IllegalStateException("native new-user app-data batch absent");
            Parcel parcel = Parcel.obtain();
            try {
                parcel.unmarshall(bytes, 0, bytes.length); parcel.setDataPosition(0);
                int count = parcel.readInt();
                if (count < 0 || count > bytes.length / 24) throw new IllegalStateException("native new-user batch count invalid");
                String[] volumes = new String[count]; String[] names = new String[count];
                int[] flags = new int[count]; int[] apps = new int[count];
                String[] seInfos = new String[count]; int[] targets = new int[count];
                for (int i = 0; i < count; i++) {
                    volumes[i] = parcel.readString(); names[i] = parcel.readString();
                    flags[i] = parcel.readInt(); apps[i] = parcel.readInt();
                    seInfos[i] = parcel.readString(); targets[i] = parcel.readInt();
                }
                if (parcel.dataAvail() != 0) throw new IllegalStateException("native new-user batch tail");
                NativeUserAppData.create(installer, user, volumes, names, flags, apps, seInfos, targets);
            } finally { parcel.recycle(); }
            host.finishUserCreation(user);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void onNewUserCreated(int user, boolean converted) {
        permissions.onNewUserCreated(user, converted);
    }
    @Override public void cleanUpUser(UserManagerService userManager, int user) {
        if (userManager == null || userManager != UserManagerService.getInstance()) {
            throw new IllegalArgumentException("foreign original UserManagerService");
        }
        try {
            synchronized (packagesLock) {
                host.cleanUpUserSettings(user);
                permissions.onUserRemoved(user);
            }
            host.finishUserRemoval(user);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public Computer snapshotComputer() { return Objects.requireNonNull(computers.get(), "native Computer unavailable"); }
    @Override public boolean hasSystemFeature(String name, int version) {
        try { return host.hasSystemFeature(name, version); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public boolean isDeviceUpgrading() {
        try { return host.isDeviceUpgrading(); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void addCrossProfileIntentFilter(Computer snapshot, WatchedIntentFilter filter,
            String owner, int source, int target, int flags) {
        Objects.requireNonNull(snapshot);
        try { host.addCrossProfileIntentFilter(Objects.requireNonNull(filter).getIntentFilter(),
                owner, source, target, flags); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public Object packageLock() { return packagesLock; }
    @Override public void readPermissionStateForUser(int user) {
        if (!Thread.holdsLock(packagesLock)) throw new IllegalStateException("native permission read lacks shared package lock");
        try { host.readPermissionStateForUser(user); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public void clearDomainUser(int user) {
        try { host.clearDomainUser(user); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    public PackagePermissionLifecycle permissionLifecycle() { return permissions; }
}
