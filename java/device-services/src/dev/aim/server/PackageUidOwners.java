package dev.aim.server;

import android.os.Parcel;
import android.os.RemoteException;
import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.SharedUserApi;
import java.io.IOException;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;

/** Immutable registered SettingBase selections, including distinct retained package instances. */
public final class PackageUidOwners {
    private PackageUidOwners() {}

    static Map<Integer, Object> capture(IPackageComputer endpoint, long version,
            Map<String, PackageState> packages, Map<String, SharedUserApi> groups,
            boolean crossUserSuspensions) throws IOException, RemoteException {
        int length = endpoint.getUidOwnerRegistryLength();
        if (length < 12 || (length & 3) != 0) throw new IOException("invalid UID registry length");
        byte[] bytes = new byte[length];
        for (int offset = 0; offset < length;) {
            int count = Math.min(65536, length - offset);
            byte[] chunk = endpoint.getUidOwnerRegistryChunk(offset, count);
            if (chunk == null || chunk.length != count) throw new IOException("incomplete UID registry chunk");
            System.arraycopy(chunk, 0, bytes, offset, count); offset += count;
        }
        return read(bytes, version, packages, groups, crossUserSuspensions);
    }

    static Map<Integer, Object> read(byte[] bytes, long version,
            Map<String, PackageState> packages, Map<String, SharedUserApi> groups,
            boolean crossUserSuspensions) throws IOException {
        var in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            if (in.readLong() != version) throw new IOException("UID registry version differs");
            int count = in.readInt();
            if (count < 0 || count > in.dataAvail() / 12) throw new IOException("invalid UID registry count");
            var result = new LinkedHashMap<Integer, Object>();
            int previous = -1;
            for (int i = 0; i < count; i++) {
                int appId = in.readInt();
                if (appId <= previous) throw new IOException("duplicate or unordered UID slot");
                previous = appId;
                int kind = in.readInt();
                String name = Objects.requireNonNull(in.readString());
                Object owner;
                switch (kind) {
                    case 1 -> {
                        PackageState state = packages.get(name);
                        if (!(state instanceof PackageStateInternal) || state.getAppId() != appId)
                            throw new IOException("UID package reference differs");
                        owner = state;
                    }
                    case 2 -> {
                        SharedUserApi group = groups.get(name);
                        if (group == null || group.getAppId() != appId) throw new IOException("UID group reference differs");
                        owner = group;
                    }
                    case 3 -> owner = RetainedPackageData.detached(
                            Objects.requireNonNull(in.createByteArray()), version, name, appId)
                            .newReplica(crossUserSuspensions);
                    default -> throw new IOException("unknown UID slot kind");
                }
                result.put(appId, owner);
            }
            if (in.dataAvail() != 0) throw new IOException("UID registry has trailing data");
            return Collections.unmodifiableMap(result);
        } catch (IllegalArgumentException | NullPointerException failure) {
            throw new IOException("invalid UID registry record", failure);
        } finally { in.recycle(); }
    }
}
