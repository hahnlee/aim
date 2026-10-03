package dev.aim.server;

import android.os.Parcel;
import android.os.Parcelable;
import java.util.Objects;

/** Code envelope carried by the snapshot's byte pages; other signing owners are separate. */
public final class PackageCode implements Parcelable {
    private final long version;
    private final String name;
    private final byte[] cache;
    private final byte[][] certificates;
    private final int[] capabilities;

    private PackageCode(Parcel in) {
        version = in.readLong();
        name = Objects.requireNonNull(in.readString());
        cache = Objects.requireNonNull(in.createByteArray());
        int count = in.readInt();
        if (count < -1 || count > in.dataAvail() / 8) {
            throw new IllegalArgumentException("invalid lineage count");
        }
        certificates = count == -1 ? null : new byte[count][];
        capabilities = count == -1 ? null : new int[count];
        for (int i = 0; i < count; i++) {
            certificates[i] = Objects.requireNonNull(in.createByteArray());
            capabilities[i] = in.readInt();
        }
    }

    public long getVersion() { return version; }
    public String getPackageName() { return name; }

    public byte[] getCache() { return cache.clone(); }
    public int[] getCapabilities() { return capabilities == null ? null : capabilities.clone(); }
    public byte[][] getCertificates() {
        if (certificates == null) return null;
        byte[][] copy = new byte[certificates.length][];
        for (int i = 0; i < copy.length; i++) copy[i] = certificates[i].clone();
        return copy;
    }

    @Override
    public void writeToParcel(Parcel out, int flags) {
        out.writeLong(version);
        out.writeString(name);
        out.writeByteArray(cache);
        out.writeInt(certificates == null ? -1 : certificates.length);
        if (certificates != null) {
            for (int i = 0; i < certificates.length; i++) {
                out.writeByteArray(certificates[i]);
                out.writeInt(capabilities[i]);
            }
        }
    }

    @Override
    public int describeContents() { return 0; }

    public static final Parcelable.Creator<PackageCode> CREATOR = new Parcelable.Creator<>() {
        @Override
        public PackageCode createFromParcel(Parcel in) { return new PackageCode(in); }
        @Override
        public PackageCode[] newArray(int size) { return new PackageCode[size]; }
    };
}
