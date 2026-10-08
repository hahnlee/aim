package dev.aim.server;

import android.content.pm.PackageInstaller;
import android.content.pm.PackageManager;
import android.os.Parcel;
import android.os.RemoteException;
import android.os.UserHandle;
import android.util.ArrayMap;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.PackageState;
import java.io.IOException;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.Objects;
import java.util.TreeSet;

/** InstallPackageHelper's permission leaves, android-16.0.0_r1 (AOSP, Apache-2.0). */
public final class InstallerPermissionBridge extends IInstallerPermissionBridge.Stub {
    /** The native PM facade installs a complete transaction view for owner reads. */
    public interface CandidateScope {
        AutoCloseable enter(IPackageScanSnapshot candidate, IPackageScanSnapshot previous)
                throws RemoteException, IOException;
        AutoCloseable restore(IPackageScanSnapshot current, java.util.Set<String> excluded)
                throws RemoteException, IOException;
    }
    private final CandidateScope scope;
    public InstallerPermissionBridge(CandidateScope scope) {
        this.scope = Objects.requireNonNull(scope);
    }
    private record Install(PackageState state, AndroidPackage oldCode, int previousAppId,
            int user, boolean instant,
            PermissionManagerServiceInternal.PackageInstalledParams params) {}

    @Override public void rollback(IPackageScanSnapshot candidate, IPackageScanSnapshot current,
            String[] packageNames, boolean crossUserSuspensions) throws RemoteException {
        Bridge.enforceSystemUid();
        var permissions = Objects.requireNonNull(LocalServices.getService(PermissionManagerServiceInternal.class));
        var names = new HashSet<String>();
        for (String name : Objects.requireNonNull(packageNames)) {
            if (!names.add(Objects.requireNonNull(name))) throw new IllegalArgumentException("duplicate rollback package");
        }
        try (var attempted = new PackageScanLease(candidate); var retained = new PackageScanLease(current)) {
            var codes = new ArrayList<AndroidPackage>();
            for (String name : names) {
                var state = Objects.requireNonNull(attempted.getPackageStateReplica(name, crossUserSuspensions));
                codes.add(Objects.requireNonNull(state.getAndroidPackage()));
            }
            // AccessPolicy.onPackageRemoved requires absence in the canonical
            // package map; this is a retained current view with only attempted
            // names withdrawn, not a mutation of the live native store.
            try (var view = scope.restore(current, names)) {
                Objects.requireNonNull(view, "permission removal scope unavailable");
                for (var code : codes) permissions.onPackageRemoved(code);
            }
            try (var view = scope.restore(current, java.util.Collections.emptySet())) {
                Objects.requireNonNull(view, "permission restoration scope unavailable");
                for (String name : names) {
                    var restored = retained.getPackageStateReplica(name, crossUserSuspensions);
                    if (restored != null && restored.getAndroidPackage() != null)
                        permissions.onPackageAdded(restored,
                                restored.getUserStateOrDefault(UserHandle.USER_SYSTEM).isInstantApp(), null);
                }
            }
        } catch (RemoteException failure) { throw failure; }
        catch (Exception failure) {
            android.util.Log.e("InstallerPermissionBridge", "Original permission rollback failed", failure);
            throw new IllegalStateException("original permission rollback failed", failure);
        }
    }

    @Override public byte[] prepare(IPackageScanSnapshot candidate,
            IPackageScanSnapshot previous, byte[] installation, boolean crossUserSuspensions)
            throws RemoteException {
        Bridge.enforceSystemUid();
        var permissions = Objects.requireNonNull(LocalServices.getService(
                PermissionManagerServiceInternal.class), "permission owner unavailable");
        var userOwner = Objects.requireNonNull(LocalServices.getService(UserManagerInternal.class),
                "user owner unavailable");
        int[] users = userOwner.getUserIds();
        var knownUsers = new HashSet<Integer>();
        for (int user : users) if (user < 0 || !knownUsers.add(user))
            throw new IllegalStateException("invalid user owner inventory");
        if (knownUsers.isEmpty()) throw new IllegalStateException("empty user owner inventory");
        Objects.requireNonNull(installation);
        Parcel in = Parcel.obtain(), out = Parcel.obtain();
        try (var added = new PackageScanLease(candidate);
                var before = new PackageScanLease(previous)) {
            in.unmarshall(installation, 0, installation.length); in.setDataPosition(0);
            int count = in.readInt();
            if (count <= 0 || count > in.dataAvail() / 28)
                throw new IllegalArgumentException("invalid permission install inventory");
            var installs = new ArrayList<Install>();
            var names = new HashSet<String>();
            var appIds = new TreeSet<Integer>();
            for (int i = 0; i < count; i++) {
                String name = Objects.requireNonNull(in.readString());
                int flags = in.readInt(), previousAppId = in.readInt(), user = in.readInt();
                if (!names.add(name) || (user != UserHandle.USER_ALL && !knownUsers.contains(user)))
                    throw new IllegalArgumentException("invalid permission installation identity");
                var state = Objects.requireNonNull(added.getPackageStateReplica(name,
                        crossUserSuspensions), "candidate package unavailable");
                var pkg = Objects.requireNonNull(state.getAndroidPackage(), "candidate code unavailable");
                var oldState = before.getPackageStateReplica(name, crossUserSuspensions);
                int expectedPrevious = oldState != null && oldState.hasSharedUser()
                        && oldState.getAppId() != state.getAppId() ? oldState.getAppId() : -1;
                if (previousAppId != expectedPrevious)
                    throw new IllegalArgumentException("previous permission app ID differs");
                var states = new ArrayMap<String, Integer>();
                String[] permissionNames = in.createStringArray();
                int[] permissionStates = in.createIntArray();
                if (permissionNames == null || permissionStates == null
                        || permissionNames.length != permissionStates.length)
                    throw new IllegalArgumentException("invalid permission state map");
                for (int p = 0; p < permissionNames.length; p++) {
                    String permission = Objects.requireNonNull(permissionNames[p]);
                    if (states.containsKey(permission))
                        throw new IllegalArgumentException("duplicate permission state");
                    states.put(permission, permissionStates[p]);
                }
                if ((flags & PackageManager.INSTALL_GRANT_ALL_REQUESTED_PERMISSIONS) != 0) {
                    states.clear();
                    for (String permission : pkg.getRequestedPermissions()) states.put(permission,
                            PackageInstaller.SessionParams.PERMISSION_STATE_GRANTED);
                }
                String[] allowlisted = in.createStringArray();
                var params = new PermissionManagerServiceInternal.PackageInstalledParams.Builder();
                params.setPermissionStates(states);
                if ((flags & PackageManager.INSTALL_ALL_WHITELIST_RESTRICTED_PERMISSIONS) != 0) {
                    params.setAllowlistedRestrictedPermissions(new java.util.ArrayList<>(pkg.getRequestedPermissions()));
                } else if (allowlisted != null) {
                    for (String permission : allowlisted) Objects.requireNonNull(permission);
                    params.setAllowlistedRestrictedPermissions(java.util.Arrays.asList(allowlisted));
                }
                params.setAutoRevokePermissionsMode(in.readInt());
                installs.add(new Install(state, oldState == null ? null : oldState.getAndroidPackage(),
                        previousAppId, user, (flags & PackageManager.INSTALL_INSTANT_APP) != 0,
                        params.build()));
                appIds.add(state.getAppId());
                if (previousAppId >= 0) {
                    for (String sibling : candidate.getPackageNames(false)) {
                        var siblingState = added.getPackageStateReplica(sibling, crossUserSuspensions);
                        if (siblingState != null && siblingState.getAppId() == previousAppId) {
                            appIds.add(previousAppId); break;
                        }
                    }
                }
            }
            if (in.dataAvail() != 0) throw new IllegalArgumentException("permission install trailing data");
            try (var transaction = scope.enter(candidate, previous)) {
                Objects.requireNonNull(transaction, "native candidate scope unavailable");
                for (var install : installs) permissions.onPackageAdded(install.state(),
                        install.instant(), install.oldCode());
                for (var install : installs) permissions.onPackageInstalled(
                        install.state().getAndroidPackage(), install.previousAppId(),
                        install.params(), install.user());
                out.writeIntArray(users); out.writeInt(appIds.size());
                for (int appId : appIds) {
                    PackageLegacyPermissions.validate(appId, users);
                    out.writeInt(appId);
                    out.writeByteArray(PackageLegacyPermissions.capture(appId, users,
                            permissions.getLegacyPermissionState(appId)));
                }
                return out.marshall();
            }
        } catch (RemoteException failure) {
            throw failure;
        } catch (RuntimeException failure) {
            android.util.Log.e("InstallerPermissionBridge", "Original permission PREPARE failed", failure);
            throw failure;
        } catch (Exception failure) {
            throw new IllegalStateException("native install permission preparation failed", failure);
        } finally { in.recycle(); out.recycle(); }
    }
}
