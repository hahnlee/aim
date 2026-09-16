package android.net;

import android.os.Parcel;
import android.os.Parcelable;

public final class NetworkCapabilities implements Parcelable {
    public static final int TRANSPORT_WIFI = 1;
    public static final int TRANSPORT_CELLULAR = 0;
    public static final int TRANSPORT_ETHERNET = 3;
    public static final int NET_CAPABILITY_INTERNET = 12;
    public static final int NET_CAPABILITY_NOT_METERED = 11;
    public static final int NET_CAPABILITY_NOT_RESTRICTED = 13;
    public static final int NET_CAPABILITY_TRUSTED = 14;
    public static final int NET_CAPABILITY_NOT_VPN = 15;
    public static final int NET_CAPABILITY_NOT_ROAMING = 18;
    public static final int NET_CAPABILITY_FOREGROUND = 19;
    public static final int NET_CAPABILITY_NOT_CONGESTED = 20;
    public static final int NET_CAPABILITY_NOT_SUSPENDED = 21;
    public static final int NET_CAPABILITY_VALIDATED = 16;
    public static final Creator<NetworkCapabilities> CREATOR = new Creator<NetworkCapabilities>() {
        public NetworkCapabilities createFromParcel(Parcel source) {
            NetworkCapabilities value = new NetworkCapabilities();
            value.transports = source.readInt();
            value.capabilities = source.readInt();
            return value;
        }
        public NetworkCapabilities[] newArray(int size) { return new NetworkCapabilities[size]; }
    };
    private int transports;
    private int capabilities;
    public NetworkCapabilities addTransportType(int transport) {
        transports |= 1 << transport;
        return this;
    }
    public NetworkCapabilities addCapability(int capability) {
        capabilities |= 1 << capability;
        return this;
    }
    public boolean hasCapability(int capability) {
        return (capabilities & (1 << capability)) != 0;
    }
    public boolean hasTransport(int transport) { return (transports & (1 << transport)) != 0; }
    boolean satisfies(NetworkCapabilities request) {
        return (request.transports & ~transports) == 0
                && (request.capabilities & ~capabilities) == 0;
    }
    public void writeToParcel(Parcel dest, int flags) {
        dest.writeInt(transports);
        dest.writeInt(capabilities);
    }
}
