package android.net;

import android.os.Parcel;
import android.os.Parcelable;

public final class Network implements Parcelable {
    public static final Creator<Network> CREATOR = new Creator<Network>() {
        public Network createFromParcel(Parcel source) { return new Network(source.readInt()); }
        public Network[] newArray(int size) { return new Network[size]; }
    };

    private final int netId;

    public Network(int id) { netId = id; }
    public int getNetId() { return netId; }
    public static Network fromNetworkHandle(long handle) {
        if ((handle & 0xffffffffL) != 0xcafed00dL) {
            throw new IllegalArgumentException("invalid network handle");
        }
        return new Network((int) (handle >>> 32));
    }
    public long getNetworkHandle() { return ((long) netId << 32) | 0xcafed00dL; }
    public void writeToParcel(Parcel dest, int flags) { dest.writeInt(netId); }
    @Override public boolean equals(Object other) {
        return other instanceof Network && ((Network) other).netId == netId;
    }
}
