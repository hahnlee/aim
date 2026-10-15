package dev.aim.server;

import android.content.pm.UserInfo;
import android.os.Binder;
import android.os.Parcel;
import java.util.Objects;
import com.android.server.pm.UserManagerService;
import java.util.List;
import java.util.function.IntPredicate;

/** Settings.getAllUsers and isAdbInstallDisallowed at the original user owner. */
public final class PackageScanUsers {
    // UserManager.DISALLOW_DEBUGGING_FEATURES at the pinned image version.
    private static final String ADB_RESTRICTION = "no_debugging_features";

    public static byte[] capture() {
        var owner = UserManagerService.getInstance();
        if (owner == null) return capture(null, id -> { throw new IllegalStateException("missing user owner"); });
        long identity = Binder.clearCallingIdentity();
        try {
            return capture(Objects.requireNonNull(owner.getUsers(true, false, false)),
                id -> owner.hasUserRestriction(ADB_RESTRICTION, id));
        } finally { Binder.restoreCallingIdentity(identity); }
    }

    public static byte[] capture(List<UserInfo> users, IntPredicate adbDisallowed) {
        Parcel out = Parcel.obtain();
        try {
            out.writeBoolean(users != null);
            if (users != null) {
                out.writeInt(users.size());
                int previous = -1;
                for (var user : users) {
                    if (user == null || user.id <= previous) throw new IllegalArgumentException("invalid original user inventory");
                    out.writeInt(user.id); out.writeBoolean(user.preCreated);
                    out.writeBoolean(adbDisallowed.test(user.id));
                    previous = user.id;
                }
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    private PackageScanUsers() {}
}
