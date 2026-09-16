package dev.darwinart.runtime.connectivity;

import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.ArrayList;

/** Pure Android-facing projection of one immutable host connectivity snapshot. */
public final class ConnectivityProjection {
    public static final int ANDROID_NETWORK_ID = 1;
    private static final long NETWORK_HANDLE_MAGIC = 0xcafed00dL;
    private static final long ANDROID_NETWORK_HANDLE =
            ((long) ANDROID_NETWORK_ID << 32) | NETWORK_HANDLE_MAGIC;
    private static final int INTERFACE_WIFI = 1 << 0;
    private static final int INTERFACE_WIRED = 1 << 1;
    private static final int INTERFACE_CELLULAR = 1 << 2;
    private static final int TYPE_MOBILE = 0;
    private static final int TYPE_WIFI = 1;
    private static final int TYPE_ETHERNET = 9;
    private static final int SUBTYPE_UNKNOWN = 0;
    private static final String SUBTYPE_NAME = "";

    public Network activeNetwork(ConnectivitySnapshot snapshot) {
        return active(snapshot) ? networkForIncarnation(ANDROID_NETWORK_ID) : null;
    }

    Network activeNetwork(ConnectivitySnapshot snapshot, int incarnation) {
        return active(snapshot) ? networkForIncarnation(incarnation) : null;
    }

    /** Returns the Android Network handle for a system-server-owned incarnation. */
    Network networkForIncarnation(int incarnation) {
        if (incarnation <= 0) throw new IllegalArgumentException("incarnation");
        long handle = ((long) incarnation << 32) | NETWORK_HANDLE_MAGIC;
        return Network.fromNetworkHandle(handle);
    }

    public NetworkInfo activeNetworkInfo(ConnectivitySnapshot snapshot) {
        return activeNetworkInfo(snapshot, ANDROID_NETWORK_ID);
    }

    NetworkInfo activeNetworkInfo(ConnectivitySnapshot snapshot, int incarnation) {
        if (!active(snapshot)) return null;
        int type = legacyType(snapshot.interfaceMask());
        NetworkInfo info = new NetworkInfo(
                type, SUBTYPE_UNKNOWN, legacyTypeName(type), SUBTYPE_NAME);
        info.setDetailedState(NetworkInfo.DetailedState.CONNECTED, null, null);
        return info;
    }

    public Network[] allNetworks(ConnectivitySnapshot snapshot) {
        Network network = activeNetwork(snapshot);
        return network == null ? new Network[0] : new Network[] {network};
    }

    Network[] allNetworks(ConnectivitySnapshot snapshot, int incarnation) {
        Network network = activeNetwork(snapshot, incarnation);
        return network == null ? new Network[0] : new Network[] {network};
    }

    public LinkProperties linkProperties(ConnectivitySnapshot snapshot, Network requested) {
        return projectLinkProperties(snapshot, requested, false, ANDROID_NETWORK_ID);
    }

    LinkProperties linkProperties(ConnectivitySnapshot snapshot, Network requested,
            int incarnation) {
        return projectLinkProperties(snapshot, requested, true, incarnation);
    }

    public NetworkCapabilities networkCapabilities(ConnectivitySnapshot snapshot,
            Network requested) {
        if (!known(snapshot, requested)) return null;
        NetworkCapabilities capabilities = new NetworkCapabilities();
        int interfaces = snapshot.interfaceMask();
        if ((interfaces & INTERFACE_WIFI) != 0) {
            capabilities.addTransportType(NetworkCapabilities.TRANSPORT_WIFI);
        }
        if ((interfaces & INTERFACE_WIRED) != 0) {
            capabilities.addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET);
        }
        if ((interfaces & INTERFACE_CELLULAR) != 0) {
            capabilities.addTransportType(NetworkCapabilities.TRANSPORT_CELLULAR);
        }
        capabilities.addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_RESTRICTED)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_TRUSTED)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_ROAMING)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_FOREGROUND)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_CONGESTED)
                .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_SUSPENDED);
        if (!snapshot.isMetered()) {
            capabilities.addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED);
        }
        if (snapshot.isValidated()) {
            capabilities.addCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED);
        }
        return capabilities;
    }

    NetworkCapabilities networkCapabilities(ConnectivitySnapshot snapshot, Network requested,
            int incarnation) {
        if (!known(snapshot, requested, incarnation)) return null;
        return networkCapabilities(snapshot, requested);
    }

    private static LinkProperties projectLinkProperties(ConnectivitySnapshot snapshot,
            Network requested, boolean requireIncarnation, int incarnation) {
        if (requireIncarnation
                ? !known(snapshot, requested, incarnation) : !known(snapshot, requested)) {
            return null;
        }
        LinkProperties result = new LinkProperties();
        String interfaceName = snapshot.interfaceName();
        if (interfaceName != null && !interfaceName.isEmpty()) {
            result.setInterfaceName(interfaceName);
        }
        // The host boundary admits numeric addresses only. Parsing a numeric literal never
        // consults DNS, and malformed provider data is omitted rather than becoming policy.
        ArrayList<InetAddress> dnsServers = new ArrayList<>();
        for (String address : snapshot.dnsServers()) {
            try {
                dnsServers.add(InetAddress.getByName(address));
            } catch (UnknownHostException ignored) {
                // A host-provider record that is not a numeric address is not truthful Android
                // LinkProperties data. Scoped/VPN resolver policy is intentionally not flattened.
            }
        }
        result.setDnsServers(dnsServers);
        return result;
    }

    private static boolean active(ConnectivitySnapshot snapshot) {
        return snapshot != null && snapshot.hasActiveNetwork();
    }

    private static boolean known(ConnectivitySnapshot snapshot, Network network) {
        return active(snapshot) && network != null
                && (network.getNetworkHandle() & 0xffffffffL) == NETWORK_HANDLE_MAGIC
                && (network.getNetworkHandle() >>> 32) > 0;
    }

    private static boolean known(ConnectivitySnapshot snapshot, Network network, int incarnation) {
        return known(snapshot, network)
                && (int) (network.getNetworkHandle() >>> 32) == incarnation;
    }

    private static int legacyType(int interfaces) {
        if ((interfaces & INTERFACE_WIFI) != 0) return TYPE_WIFI;
        if ((interfaces & INTERFACE_WIRED) != 0) return TYPE_ETHERNET;
        if ((interfaces & INTERFACE_CELLULAR) != 0) return TYPE_MOBILE;
        return TYPE_ETHERNET;
    }

    private static String legacyTypeName(int type) {
        if (type == TYPE_WIFI) return "WIFI";
        if (type == TYPE_MOBILE) return "MOBILE";
        return "ETHERNET";
    }
}
