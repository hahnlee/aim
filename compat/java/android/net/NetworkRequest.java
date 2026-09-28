package android.net;

import android.os.Parcel;
import android.os.Parcelable;

import java.util.ArrayList;

/** Immutable network selector matching the public Android framework contract. */
public final class NetworkRequest implements Parcelable {
    public static final Creator<NetworkRequest> CREATOR = new Creator<NetworkRequest>() {
        @Override
        public NetworkRequest createFromParcel(Parcel source) {
            return new NetworkRequest(source.readLong(), source.readLong());
        }

        @Override
        public NetworkRequest[] newArray(int size) {
            return new NetworkRequest[size];
        }
    };

    private final long capabilities;
    private final long transports;

    private NetworkRequest(long capabilities, long transports) {
        this.capabilities = capabilities;
        this.transports = transports;
    }

    public boolean hasCapability(int capability) {
        return contains(capabilities, capability);
    }

    public boolean hasTransport(int transport) {
        return contains(transports, transport);
    }

    public int[] getCapabilities() {
        return unpack(capabilities);
    }

    public int[] getTransportTypes() {
        return unpack(transports);
    }

    public boolean canBeSatisfiedBy(NetworkCapabilities candidate) {
        if (candidate == null) return false;
        for (int capability : getCapabilities()) {
            if (!candidate.hasCapability(capability)) return false;
        }
        int[] requestedTransports = getTransportTypes();
        if (requestedTransports.length == 0) return true;
        for (int transport : requestedTransports) {
            if (candidate.hasTransport(transport)) return true;
        }
        return false;
    }

    @Override
    public int describeContents() {
        return 0;
    }

    @Override
    public void writeToParcel(Parcel destination, int flags) {
        destination.writeLong(capabilities);
        destination.writeLong(transports);
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof NetworkRequest)) return false;
        NetworkRequest request = (NetworkRequest) other;
        return capabilities == request.capabilities && transports == request.transports;
    }

    @Override
    public int hashCode() {
        return Long.hashCode(capabilities) * 31 + Long.hashCode(transports);
    }

    private static boolean contains(long bits, int value) {
        return value >= 0 && value < Long.SIZE && (bits & (1L << value)) != 0;
    }

    private static int[] unpack(long bits) {
        ArrayList<Integer> values = new ArrayList<>();
        for (int value = 0; value < Long.SIZE; value++) {
            if ((bits & (1L << value)) != 0) values.add(value);
        }
        int[] result = new int[values.size()];
        for (int index = 0; index < result.length; index++) result[index] = values.get(index);
        return result;
    }

    public static final class Builder {
        private long capabilities = (1L << NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED)
                | (1L << NetworkCapabilities.NET_CAPABILITY_TRUSTED)
                | (1L << NetworkCapabilities.NET_CAPABILITY_NOT_VPN);
        private long transports;

        public Builder() {}

        public Builder(NetworkRequest request) {
            if (request == null) throw new NullPointerException("request");
            capabilities = request.capabilities;
            transports = request.transports;
        }

        public Builder addCapability(int capability) {
            capabilities |= bit(capability);
            return this;
        }

        public Builder removeCapability(int capability) {
            capabilities &= ~bit(capability);
            return this;
        }

        public Builder clearCapabilities() {
            capabilities = 0;
            return this;
        }

        public Builder addTransportType(int transport) {
            transports |= bit(transport);
            return this;
        }

        public Builder removeTransportType(int transport) {
            transports &= ~bit(transport);
            return this;
        }

        public NetworkRequest build() {
            return new NetworkRequest(capabilities, transports);
        }

        private static long bit(int value) {
            if (value < 0 || value >= Long.SIZE) {
                throw new IllegalArgumentException("value out of range: " + value);
            }
            return 1L << value;
        }
    }
}
