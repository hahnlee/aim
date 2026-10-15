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
    public record Member(String name, RetainedPackageData retained) {}
    private final List<Member> members;

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
        var values = new ArrayList<Member>(count);
        var unique = new HashSet<String>();
        for (int i = 0; i < count; i++) {
            String member = Objects.requireNonNull(in.readString());
            int kind = in.readInt();
            if (kind != 0 && kind != 1) throw new IllegalArgumentException("invalid shared member kind");
            if (!unique.add(kind + ":" + member)) throw new IllegalArgumentException("duplicate shared UID instance");
            var retained = kind == 1 ? new RetainedPackageData(in.createByteArray(), version, member, appId, name) : null;
            values.add(new Member(member, retained));
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
    public List<String> getPackageNames() { return members.stream().map(Member::name).toList(); }
    public List<Member> getMembers() { return members; }

    @Override public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version); out.writeString(name); out.writeInt(appId);
        out.writeBoolean(privileged); out.writeInt(liveSdk); out.writeInt(snapshotSdk);
        PackageSigningState.Signing.write(out, signing);
        out.writeInt(members.size());
        for (var member : members) {
            out.writeString(member.name()); out.writeBoolean(member.retained() != null);
            if (member.retained() != null) member.retained().writeToParcel(out);
        }
    }
    @Override public int describeContents() { return 0; }
    public static final Parcelable.Creator<SharedUserData> CREATOR = new Parcelable.Creator<>() {
        @Override public SharedUserData createFromParcel(Parcel in) { return new SharedUserData(in); }
        @Override public SharedUserData[] newArray(int size) { return new SharedUserData[size]; }
    };
}
