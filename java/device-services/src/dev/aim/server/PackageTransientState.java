package dev.aim.server;

import android.os.Parcel;
import android.os.Parcelable;
import java.util.Objects;

/** Immutable PackageStateUnserialized flags and APEX owner in one capture. */
public final class PackageTransientState implements Parcelable {
    private final long version;
    private final String name;
    private final int appId;
    private final boolean factory;
    private final boolean hiddenUntilInstalled;
    private final boolean updatedSystemApp;
    private final boolean apkInUpdatedApex;
    private final String apexModuleName;

    private PackageTransientState(Parcel in) {
        if (in.dataAvail() < 8) throw new IllegalArgumentException("missing transient version");
        version = in.readLong(); name = Objects.requireNonNull(string(in));
        if (in.dataAvail() < 24) throw new IllegalArgumentException("incomplete transient setting");
        appId = in.readInt(); factory = bool(in);
        hiddenUntilInstalled = bool(in); updatedSystemApp = bool(in); apkInUpdatedApex = bool(in);
        apexModuleName = string(in);
    }
    private static String string(Parcel in) {
        if (in.dataAvail() < 4) throw new IllegalArgumentException("missing transient string");
        int at = in.dataPosition(); int length = in.readInt(); in.setDataPosition(at);
        long bytes = length == -1 ? 4 : 4 + (((long) length + 1) * 2 + 3) / 4 * 4;
        if (length < -1 || bytes > in.dataAvail()) throw new IllegalArgumentException("incomplete transient string");
        String value = in.readString();
        if (length != -1 && (value == null || value.length() != length)) throw new IllegalArgumentException("invalid transient string");
        return value;
    }
    private static boolean bool(Parcel in) {
        int value = in.readInt();
        if (value != 0 && value != 1) throw new IllegalArgumentException("invalid transient boolean");
        return value == 1;
    }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public int getAppId() { return appId; }
    public boolean isFactory() { return factory; }
    public boolean isHiddenUntilInstalled() { return hiddenUntilInstalled; }
    public boolean isUpdatedSystemApp() { return updatedSystemApp; }
    public boolean isApkInUpdatedApex() { return apkInUpdatedApex; }
    public String getApexModuleName() { return apexModuleName; }
    @Override public int describeContents() { return 0; }
    @Override public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version); out.writeString(name); out.writeInt(appId); out.writeBoolean(factory);
        out.writeBoolean(hiddenUntilInstalled); out.writeBoolean(updatedSystemApp);
        out.writeBoolean(apkInUpdatedApex); out.writeString(apexModuleName);
    }
    public static final Parcelable.Creator<PackageTransientState> CREATOR = new Parcelable.Creator<>() {
        @Override public PackageTransientState createFromParcel(Parcel in) { return new PackageTransientState(in); }
        @Override public PackageTransientState[] newArray(int size) { return new PackageTransientState[size]; }
    };
}
