package dev.aim.server;

import android.os.Parcel;
import com.android.server.pm.pkg.PackageState;

/** Sparse user inventory and actual factory-to-active object aliases. */
public final class PackageUserScopeFeed {
    public static byte[] capture(PackageState state, PackageState active, boolean factory) {
        Parcel out = Parcel.obtain();
        try {
            out.writeString(state.getPackageName()); out.writeInt(state.getAppId());
            out.writeString(state.getPath().getPath()); out.writeLong(state.getVersionCode());
            out.writeBoolean(factory);
            var users = state.getUserStates();
            var activeUsers = active == null ? null : active.getUserStates();
            out.writeInt(users.size());
            for (int i = 0; i < users.size(); i++) {
                int id = users.keyAt(i);
                if (id < 0 || users.valueAt(i) == null) throw new IllegalArgumentException("invalid original user state");
                out.writeInt(id);
                out.writeBoolean(factory && activeUsers != null && users.valueAt(i) == activeUsers.get(id));
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    private PackageUserScopeFeed() {}
}
