package dev.aim.server;

import android.os.Parcel;
import android.os.Parcelable;
import java.util.Objects;

/** Immutable boot label and its original base/override destination. */
public final class PackageSeInfoState implements Parcelable {
    private final long version;
    private final String name;
    private final String label;
    private final boolean override;

    private PackageSeInfoState(Parcel in) {
        version = in.readLong();
        name = Objects.requireNonNull(in.readString());
        label = Objects.requireNonNull(in.readString());
        override = in.readBoolean();
    }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public String getLabel() { return label; }
    public boolean isOverride() { return override; }
    @Override public int describeContents() { return 0; }
    @Override public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version);
        out.writeString(name);
        out.writeString(label);
        out.writeBoolean(override);
    }
    public static final Parcelable.Creator<PackageSeInfoState> CREATOR = new Parcelable.Creator<>() {
        @Override public PackageSeInfoState createFromParcel(Parcel in) { return new PackageSeInfoState(in); }
        @Override public PackageSeInfoState[] newArray(int size) { return new PackageSeInfoState[size]; }
    };
}
