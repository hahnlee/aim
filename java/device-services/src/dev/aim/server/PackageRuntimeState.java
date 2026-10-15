package dev.aim.server;

import android.os.Parcel;
import java.util.Objects;

/** Complete immutable runtime inputs for one active or factory setting. */
public final class PackageRuntimeState {
    private final long version;
    private final String name;
    private final int appId;
    private final boolean factory;
    private final boolean code;
    private final String seInfo;
    private final String overrideSeInfo;
    private final long[] usage;
    private final PackageLibraryState libraries;

    private PackageRuntimeState(Parcel in) {
        version = in.readLong(); name = Objects.requireNonNull(in.readString()); appId = in.readInt();
        factory = bool(in); code = bool(in);
        seInfo = in.readString(); overrideSeInfo = in.readString();
        usage = Objects.requireNonNull(in.createLongArray());
        if (usage.length != 8) throw new IllegalArgumentException("runtime usage reason count differs");
        libraries = PackageLibraryState.read(in);
        if (libraries.getVersion() != version || !libraries.getPackageName().equals(name)
                || libraries.getAppId() != appId) throw new IllegalArgumentException("runtime dependency identity differs");
    }
    private static boolean bool(Parcel in) {
        int value = in.readInt();
        if (value != 0 && value != 1) throw new IllegalArgumentException("invalid runtime boolean");
        return value == 1;
    }
    public static PackageRuntimeState read(Parcel in) { return new PackageRuntimeState(in); }
    public long getVersion() { return version; }
    public String getPackageName() { return name; }
    public int getAppId() { return appId; }
    public boolean isFactory() { return factory; }
    public boolean hasCode() { return code; }
    public String getSeInfo() { return seInfo; }
    public String getOverrideSeInfo() { return overrideSeInfo; }
    public long[] getUsage() { return usage.clone(); }
    public PackageLibraryState getLibraries() { return libraries; }
}
