package dev.aim.server;

import android.os.Parcel;
import com.android.server.pm.CapturedPackageSetting;
import java.util.List;
import java.util.Objects;
import java.util.function.Function;

/** One unparsed setting instance retained by a captured UID owner. */
public final class RetainedPackageData {
    private final byte[] bytes;
    private final PackageSettingData setting;
    private final PackageSigningState signing;
    private final PackageRuntimeState runtime;
    private final PackageTransientState transientState;
    private final List<PackageUserStateData> users;

    RetainedPackageData(byte[] bytes, long version, String name, int appId, String groupName) {
        this(bytes, version, name, appId, groupName, true);
    }

    static RetainedPackageData detached(byte[] bytes, long version, String name, int appId) {
        return new RetainedPackageData(bytes, version, name, appId, null, false);
    }

    private RetainedPackageData(byte[] bytes, long version, String name, int appId,
            String groupName, boolean shared) {
        this.bytes = Objects.requireNonNull(bytes).clone();
        if (bytes.length == 0 || (bytes.length & 3) != 0) throw new IllegalArgumentException("invalid retained envelope");
        Parcel in = Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            setting = frame(in, PackageSettingData::read);
            signing = frame(in, PackageSigningState.CREATOR::createFromParcel);
            runtime = frame(in, PackageRuntimeState::read);
            transientState = frame(in, PackageTransientState.CREATOR::createFromParcel);
            int count = in.readInt();
            if (count < 0 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid retained user count");
            var values = new java.util.ArrayList<PackageUserStateData>(count);
            int previous = -1;
            for (int i = 0; i < count; i++) {
                var user = frame(in, PackageUserStateData::read);
                if (user.getVersion() != version || !user.getPackageName().equals(name)
                        || user.getAppId() != appId || user.isFactory() || user.getUserId() <= previous) {
                    throw new IllegalArgumentException("retained user identity differs");
                }
                previous = user.getUserId(); values.add(user);
            }
            users = List.copyOf(values);
            if (in.dataAvail() != 0 || setting.getVersion() != version
                    || !setting.getPackageName().equals(name) || setting.appId != appId
                    || setting.isFactory() || setting.sharedUser != shared || shared && setting.sharedUserAppId != appId
                    || !Objects.equals(signing.getSharedGroupName(), groupName)
                    || runtime.hasCode()) throw new IllegalArgumentException("retained setting identity differs");
            // Resolve every original owner before a replica can be published.
            newReplica(false);
        } finally { in.recycle(); }
    }

    private static <T> T frame(Parcel in, Function<Parcel, T> read) {
        byte[] bytes = Objects.requireNonNull(in.createByteArray());
        if (bytes.length == 0 || (bytes.length & 3) != 0) throw new IllegalArgumentException("invalid retained frame");
        Parcel value = Parcel.obtain();
        try {
            value.unmarshall(bytes, 0, bytes.length); value.setDataPosition(0);
            T result = read.apply(value);
            if (value.dataAvail() != 0) throw new IllegalArgumentException("retained frame tail");
            return result;
        } finally { value.recycle(); }
    }

    void writeToParcel(Parcel out) { out.writeByteArray(bytes); }

    public PackageStateReplica newReplica(boolean crossUserSuspensions) {
        long version = setting.getVersion();
        java.util.function.Supplier<com.android.server.pm.PackageSetting> factory = () -> {
            var value = CapturedPackageSetting.withUsers(setting, users, version, false, crossUserSuspensions);
            PackageObjects.restoreSavedSigning(value, signing, version, false);
            PackageObjects.restoreRuntime(value, runtime, version, false);
            PackageObjects.restoreTransientState(value, transientState, version, false);
            return value;
        };
        var replicas = new java.util.LinkedHashMap<Integer, PackageUserStateReplica>();
        for (var user : users) replicas.put(user.getUserId(), new PackageUserStateReplica(user, crossUserSuspensions));
        // AndroidPackageUtils returns ENABLED for a setting with no parsed code.
        return new PackageStateReplica(factory, replicas, 2);
    }
}
