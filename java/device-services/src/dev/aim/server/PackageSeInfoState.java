package dev.aim.server;

import android.os.Parcel;
import android.os.Parcelable;
import java.util.Objects;

/** Immutable original transient seInfo fields, including an explicit missing base. */
public final class PackageSeInfoState implements Parcelable {
    private final long version;
    private final String name;
    private final String base;
    private final String override;

    private PackageSeInfoState(Parcel in) {
        version = in.readLong();
        name = Objects.requireNonNull(in.readString());
        base = in.readString();
        override = in.readString();
        if (getLabel() == null) throw new IllegalArgumentException("missing seInfo label");
    }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public String getBaseLabel() { return base; }
    public String getOverrideLabel() { return override; }
    public String getLabel() { return isOverride() ? override : base; }
    public boolean isOverride() { return override != null && !override.isEmpty(); }
    @Override public int describeContents() { return 0; }
    @Override public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version);
        out.writeString(name);
        out.writeString(base);
        out.writeString(override);
    }
    public static final Parcelable.Creator<PackageSeInfoState> CREATOR = new Parcelable.Creator<>() {
        @Override public PackageSeInfoState createFromParcel(Parcel in) { return new PackageSeInfoState(in); }
        @Override public PackageSeInfoState[] newArray(int size) { return new PackageSeInfoState[size]; }
    };
}
