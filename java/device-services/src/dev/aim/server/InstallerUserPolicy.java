package dev.aim.server;

import android.app.admin.DevicePolicyManagerInternal;
import android.os.Parcel;
import android.os.UserManager;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;

/** Current original policy owners consumed by PackageInstallerService.createSessionInternal. */
public final class InstallerUserPolicy {
    private InstallerUserPolicy() {}

    public static boolean shellDebuggingRestricted(int userId) {
        if (userId < 0) throw new IllegalArgumentException("Invalid userId " + userId);
        var users = LocalServices.getService(UserManagerInternal.class);
        if (users == null) throw new IllegalStateException("shell user policy owner is unavailable");
        boolean restricted = users.hasUserRestriction(UserManager.DISALLOW_DEBUGGING_FEATURES, userId);
        if (users != LocalServices.getService(UserManagerInternal.class))
            throw new IllegalStateException("shell policy owner changed during capture");
        return restricted;
    }

    public static byte[] capture(int userId) {
        if (userId < 0) throw new IllegalArgumentException("Invalid userId " + userId);
        var users = LocalServices.getService(UserManagerInternal.class);
        var devicePolicy = LocalServices.getService(DevicePolicyManagerInternal.class);
        if (users == null) throw new IllegalStateException("installer user policy owner is unavailable");
        if (devicePolicy == null) throw new IllegalStateException("installer device policy owner is unavailable");

        boolean exists = users.exists(userId);
        boolean debugging = users.hasUserRestriction(UserManager.DISALLOW_DEBUGGING_FEATURES, userId);
        boolean installing = users.hasUserRestriction(UserManager.DISALLOW_INSTALL_APPS, userId);
        boolean managed = devicePolicy.isUserOrganizationManaged(userId);
        if (users != LocalServices.getService(UserManagerInternal.class)
                || devicePolicy != LocalServices.getService(DevicePolicyManagerInternal.class))
            throw new IllegalStateException("installer policy owner changed during capture");

        var out = Parcel.obtain();
        try {
            out.writeInt(userId);
            out.writeBoolean(exists);
            out.writeBoolean(installing);
            out.writeBoolean(debugging);
            out.writeBoolean(managed);
            return out.marshall();
        } finally { out.recycle(); }
    }
}
