// Permission lifecycle from AOSP PackageManagerService, android-16.0.0_r1.
// Copyright (C) The Android Open Source Project, Apache License 2.0.
package dev.aim.server;

import android.content.pm.UserInfo;
import android.os.Binder;
import android.os.Process;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import java.util.List;
import java.util.Objects;

/** PMS permission lifecycle at the pinned image, over the original permission owners. */
public final class PackagePermissionLifecycle {
    public interface StateOwner {
        /** The package lock shared with the original UserManagerService. */
        Object packageLock();
        /** Synchronous native Settings permission-state read with packageLock held. */
        void readPermissionStateForUser(int userId);
        void clearDomainUser(int userId);
    }

    private final PermissionManagerServiceInternal permissions;
    private final LegacyPermissionManagerInternal legacy;
    private final StateOwner state;
    private final String fingerprint;

    public PackagePermissionLifecycle(PermissionManagerServiceInternal permissions,
            LegacyPermissionManagerInternal legacy, StateOwner state, String fingerprint) {
        this.permissions = Objects.requireNonNull(permissions);
        this.legacy = Objects.requireNonNull(legacy);
        this.state = Objects.requireNonNull(state);
        this.fingerprint = Objects.requireNonNull(fingerprint);
    }

    /** Runs at PMS's permission position, after user/storage reconciliation. */
    public void systemReady(List<UserInfo> livingUsers) {
        enforceSystemOrRoot();
        Objects.requireNonNull(livingUsers);
        permissions.onSystemReady();
        // Snapshot all upgrade decisions before the first grant, as original PMS does.
        int[] upgrades = new int[livingUsers.size()];
        int count = 0;
        for (UserInfo user : livingUsers) {
            if (!Objects.equals(permissions.getDefaultPermissionGrantFingerprint(user.id),
                    fingerprint)) upgrades[count++] = user.id;
        }
        for (int i = 0; i < count; i++) grantDefaults(upgrades[i]);
        if (count == 0) legacy.scheduleReadDefaultPermissionExceptions();
    }

    public void onNewUserCreated(int userId, boolean convertedFromPreCreated) {
        enforceSystemOrRoot();
        if (!convertedFromPreCreated || !readPermissionStateForUser(userId)) {
            permissions.onUserCreated(userId);
            grantDefaults(userId);
            state.clearDomainUser(userId);
        }
    }

    public void onUserRemoved(int userId) {
        enforceSystemOrRoot();
        permissions.onUserRemoved(userId);
    }

    private boolean readPermissionStateForUser(int userId) {
        synchronized (Objects.requireNonNull(state.packageLock())) {
            permissions.writeLegacyPermissionStateTEMP();
            state.readPermissionStateForUser(userId);
            permissions.readLegacyPermissionStateTEMP();
            // This deliberately follows PMS: true means an upgrade is needed.
            return !Objects.equals(permissions.getDefaultPermissionGrantFingerprint(userId),
                    fingerprint);
        }
    }

    private void grantDefaults(int userId) {
        legacy.grantDefaultPermissions(userId);
        permissions.setDefaultPermissionGrantFingerprint(fingerprint, userId);
    }

    private static void enforceSystemOrRoot() {
        int uid = Binder.getCallingUid();
        if (uid != Process.SYSTEM_UID && uid != 0) {
            throw new SecurityException("Only the system can run package permission lifecycle");
        }
    }
}
