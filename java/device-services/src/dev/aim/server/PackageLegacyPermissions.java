package dev.aim.server;

import android.os.Parcel;
import com.android.server.pm.permission.LegacyPermissionState;
import java.util.HashSet;
import java.util.Objects;

/** Detached original permission-owner inputs for an explicitly resolved user inventory. */
public final class PackageLegacyPermissions {
    private PackageLegacyPermissions() {}

    public static byte[] capture(int appId, int[] users, LegacyPermissionState state) {
        validate(appId, users);
        Objects.requireNonNull(state);
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(appId); out.writeInt(users.length);
            for (int user : users) {
                out.writeInt(user); out.writeBoolean(state.isMissing(user));
                var permissions = state.getPermissionStates(user);
                out.writeInt(permissions.size());
                for (var permission : permissions) {
                    out.writeString(permission.getName());
                    out.writeBoolean(permission.isRuntime());
                    out.writeBoolean(permission.isGranted());
                    out.writeInt(permission.getFlags());
                }
            }
            return out.marshall();
        } finally { out.recycle(); }
    }

    public static LegacyPermissionState restore(int appId, int[] users, byte[] bytes) {
        validate(appId, users);
        Objects.requireNonNull(bytes);
        if (bytes.length % 4 != 0) throw new IllegalArgumentException("unaligned permission capture");
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            if (in.dataAvail() < 8 || in.readInt() != appId || in.readInt() != users.length) {
                throw new IllegalArgumentException("permission capture identity differs");
            }
            var state = new LegacyPermissionState();
            for (int user : users) {
                if (in.dataAvail() < 12 || in.readInt() != user) throw new IllegalArgumentException("permission user inventory differs");
                state.setMissing(readBoolean(in), user);
                int count = in.readInt();
                if (count < 0 || count > in.dataAvail() / 16) throw new IllegalArgumentException("invalid permission count");
                var names = new HashSet<String>();
                for (int i = 0; i < count; i++) {
                    String name = in.readString();
                    if (!names.add(name) || in.dataAvail() < 12) throw new IllegalArgumentException("invalid permission record");
                    boolean runtime = readBoolean(in), granted = readBoolean(in);
                    state.putPermissionState(new LegacyPermissionState.PermissionState(name, runtime, granted, in.readInt()), user);
                }
            }
            if (in.dataAvail() != 0) throw new IllegalArgumentException("permission capture has trailing data");
            if (!java.util.Arrays.equals(bytes, capture(appId, users, state))) throw new IllegalArgumentException("noncanonical permission capture");
            return state;
        } finally { in.recycle(); }
    }

    private static boolean readBoolean(Parcel in) {
        int value = in.readInt();
        if (value != 0 && value != 1) throw new IllegalArgumentException("invalid permission boolean");
        return value == 1;
    }

    public static void validate(int appId, int[] users) {
        if (appId < 0 || appId >= 100000 || users == null || users.length == 0) {
            throw new IllegalArgumentException("permission capture requires app ID and resolved users");
        }
        var seen = new HashSet<Integer>();
        for (int user : users) {
            if (user < 0 || !seen.add(user)) throw new IllegalArgumentException("invalid permission user inventory");
        }
    }
}
