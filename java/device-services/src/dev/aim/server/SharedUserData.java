package dev.aim.server;

import android.content.pm.SigningDetails;
import android.os.Parcel;
import android.os.Parcelable;
import java.util.List;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.Objects;

/** Immutable projection of one captured native shared UID owner. */
public final class SharedUserData implements Parcelable {
    private final long version;
    private final String name;
    private final int appId;
    private final boolean privileged;
    private final int liveSdk;
    private final int snapshotSdk;
    private final PackageSigningState.Signing signing;
    private final List<String> members;

    private SharedUserData(Parcel in) {
        version = in.readLong();
        name = Objects.requireNonNull(in.readString());
        appId = in.readInt();
        int flag = in.readInt();
        if (flag != 0 && flag != 1) throw new IllegalArgumentException("invalid shared UID privilege");
        privileged = flag == 1;
        liveSdk = in.readInt();
        snapshotSdk = in.readInt();
        signing = PackageSigningState.Signing.read(in);
        int count = in.readInt();
        if (count < 0 || count > in.dataAvail() / 4) throw new IllegalArgumentException("invalid shared UID member count");
        var values = new ArrayList<String>(count);
        var unique = new HashSet<String>();
        for (int i = 0; i < count; i++) {
            String member = Objects.requireNonNull(in.readString());
            if (!unique.add(member)) throw new IllegalArgumentException("duplicate shared UID member");
            values.add(member);
        }
        members = List.copyOf(values);
    }

    public long getVersion() { return version; }
    public String getName() { return name; }
    public int getAppId() { return appId; }
    public boolean isPrivileged() { return privileged; }
    public int getLiveSeInfoTargetSdkVersion() { return liveSdk; }
    public int getSeInfoTargetSdkVersion() { return snapshotSdk; }
    public SigningDetails getSigningDetails() { return PackageSigningState.Signing.details(signing); }
    public List<String> getPackageNames() { return members; }

    @Override public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version); out.writeString(name); out.writeInt(appId);
        out.writeBoolean(privileged); out.writeInt(liveSdk); out.writeInt(snapshotSdk);
        PackageSigningState.Signing.write(out, signing);
        out.writeInt(members.size());
        for (String member : members) out.writeString(member);
    }
    @Override public int describeContents() { return 0; }
    public static final Parcelable.Creator<SharedUserData> CREATOR = new Parcelable.Creator<>() {
        @Override public SharedUserData createFromParcel(Parcel in) { return new SharedUserData(in); }
        @Override public SharedUserData[] newArray(int size) { return new SharedUserData[size]; }
    };
}
