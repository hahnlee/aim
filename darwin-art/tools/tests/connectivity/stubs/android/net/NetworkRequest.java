package android.net;

import android.os.Parcel;
import android.os.Parcelable;

/** Test-only Android 16 NetworkRequest shape, including the system-owned request id. */
public final class NetworkRequest implements Parcelable {
    public static final Creator<NetworkRequest> CREATOR = new Creator<NetworkRequest>() {
        public NetworkRequest createFromParcel(Parcel source) {
            NetworkCapabilities capabilities = NetworkCapabilities.CREATOR.createFromParcel(source);
            return new NetworkRequest(capabilities, source.readInt(), source.readInt(),
                    source.readString());
        }
        public NetworkRequest[] newArray(int size) { return new NetworkRequest[size]; }
    };

    private final NetworkCapabilities capabilities;
    private final int legacyType;
    private final int requestId;
    private final String type;

    public NetworkRequest(NetworkCapabilities value, int legacy, int id, String requestType) {
        capabilities = value;
        legacyType = legacy;
        requestId = id;
        type = requestType;
    }

    public int getRequestId() { return requestId; }
    public boolean isListen() { return "LISTEN".equals(type); }
    public boolean canBeSatisfiedBy(NetworkCapabilities candidate) {
        if (candidate == null) return false;
        return candidate.satisfies(capabilities);
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof NetworkRequest)) return false;
        NetworkRequest value = (NetworkRequest) other;
        return requestId == value.requestId && legacyType == value.legacyType
                && type.equals(value.type);
    }

    @Override
    public int hashCode() { return requestId * 31 + legacyType * 7 + type.hashCode(); }

    @Override
    public void writeToParcel(Parcel destination, int flags) {
        capabilities.writeToParcel(destination, flags);
        destination.writeInt(legacyType);
        destination.writeInt(requestId);
        destination.writeString(type);
    }
}
