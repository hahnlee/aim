package android.net;

import android.os.Parcel;
import android.os.Parcelable;

/** Compile-only view of Android 16 system APIs omitted from the public SDK. */
public final class NetworkCapabilities implements Parcelable {
    public static final int TRANSPORT_CELLULAR = 0;
    public static final int TRANSPORT_WIFI = 1;
    public static final int TRANSPORT_ETHERNET = 3;
    public static final int NET_CAPABILITY_NOT_METERED = 11;
    public static final int NET_CAPABILITY_INTERNET = 12;
    public static final int NET_CAPABILITY_NOT_RESTRICTED = 13;
    public static final int NET_CAPABILITY_TRUSTED = 14;
    public static final int NET_CAPABILITY_NOT_VPN = 15;
    public static final int NET_CAPABILITY_VALIDATED = 16;
    public static final int NET_CAPABILITY_NOT_ROAMING = 18;
    public static final int NET_CAPABILITY_FOREGROUND = 19;
    public static final int NET_CAPABILITY_NOT_CONGESTED = 20;
    public static final int NET_CAPABILITY_NOT_SUSPENDED = 21;

    public static final Creator<NetworkCapabilities> CREATOR = new Creator<NetworkCapabilities>() {
        @Override public NetworkCapabilities createFromParcel(Parcel source) {
            return new NetworkCapabilities();
        }
        @Override public NetworkCapabilities[] newArray(int size) {
            return new NetworkCapabilities[size];
        }
    };

    public NetworkCapabilities() {}
    public NetworkCapabilities addTransportType(int transportType) { return this; }
    public NetworkCapabilities addCapability(int capability) { return this; }
    @Override public int describeContents() { return 0; }
    @Override public void writeToParcel(Parcel destination, int flags) {}
}
