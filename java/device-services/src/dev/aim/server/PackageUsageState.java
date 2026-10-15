package dev.aim.server;

import android.os.Parcel;
import android.os.Parcelable;
import java.util.Objects;

/** Immutable usage metadata from the same captured owner as the code/setting. */
public final class PackageUsageState implements Parcelable {
    private final long version;
    private final String name;
    private final boolean historical;
    private final long[] times;

    private PackageUsageState(Parcel in) {
        version = in.readLong();
        name = Objects.requireNonNull(in.readString());
        historical = in.readBoolean();
        times = Objects.requireNonNull(in.createLongArray());
        if (times.length != 8) throw new IllegalArgumentException("invalid usage reason count");
    }

    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public boolean isHistoricalAvailable() { return historical; }
    public long[] getLastPackageUsageTimeInMills() { return times.clone(); }
    public long getLatestPackageUseTimeInMills() {
        long latest = 0;
        for (long time : times) latest = Math.max(latest, time);
        return latest;
    }
    public long getLatestForegroundPackageUseTimeInMills() {
        return Math.max(0, Math.max(times[0], times[2]));
    }

    @Override
    public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version);
        out.writeString(name);
        out.writeBoolean(historical);
        out.writeLongArray(times);
    }
    @Override public int describeContents() { return 0; }
    public static final Parcelable.Creator<PackageUsageState> CREATOR = new Parcelable.Creator<>() {
        @Override public PackageUsageState createFromParcel(Parcel in) { return new PackageUsageState(in); }
        @Override public PackageUsageState[] newArray(int size) { return new PackageUsageState[size]; }
    };
}
