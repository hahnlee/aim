package android.net;

/** Transport properties exposed by the process-local connectivity service. */
public final class NetworkCapabilities {
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

    public static final int TRANSPORT_CELLULAR = 0;
    public static final int TRANSPORT_WIFI = 1;
    public static final int TRANSPORT_VPN = 4;

    private final long capabilities;
    private final long transports;

    public NetworkCapabilities() {
        this(false);
    }

    public NetworkCapabilities(boolean vpn) {
        capabilities = (1L << NET_CAPABILITY_INTERNET)
                | (1L << NET_CAPABILITY_NOT_METERED)
                | (1L << NET_CAPABILITY_NOT_RESTRICTED)
                | (1L << NET_CAPABILITY_TRUSTED)
                | (1L << NET_CAPABILITY_VALIDATED)
                | (1L << NET_CAPABILITY_NOT_ROAMING)
                | (1L << NET_CAPABILITY_FOREGROUND)
                | (1L << NET_CAPABILITY_NOT_CONGESTED)
                | (1L << NET_CAPABILITY_NOT_SUSPENDED)
                | (vpn ? 0 : (1L << NET_CAPABILITY_NOT_VPN));
        transports = 1L << (vpn ? TRANSPORT_VPN : TRANSPORT_WIFI);
    }

    public boolean hasCapability(int capability) {
        return contains(capabilities, capability);
    }

    public boolean hasTransport(int transportType) {
        return contains(transports, transportType);
    }

    public int getLinkDownstreamBandwidthKbps() {
        return 100000;
    }

    public int getLinkUpstreamBandwidthKbps() {
        return 100000;
    }

    private static boolean contains(long bits, int value) {
        return value >= 0 && value < Long.SIZE && (bits & (1L << value)) != 0;
    }
}
