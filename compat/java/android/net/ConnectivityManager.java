package android.net;

import android.content.Context;
import android.os.Handler;

import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executor;
import java.util.ArrayList;

/** Process-local connectivity service for the Darwin Android framework port. */
public class ConnectivityManager {
    public static final int TYPE_MOBILE = 0;
    public static final int TYPE_WIFI = 1;

    private static final Network ACTIVE_NETWORK = new Network(1);
    private static final LinkProperties ACTIVE_LINK_PROPERTIES = new LinkProperties();

    private final NetworkInfo activeNetwork =
            new NetworkInfo(TYPE_WIFI, true);
    private final CopyOnWriteArrayList<OnNetworkActiveListener> networkActiveListeners =
            new CopyOnWriteArrayList<>();

    public ConnectivityManager(Context context) {}

    public NetworkInfo getActiveNetworkInfo() {
        return activeNetwork;
    }

    /** The host's primary Ethernet/Wi-Fi route is treated as unmetered. */
    public boolean isActiveNetworkMetered() {
        return false;
    }

    public Network[] getAllNetworks() {
        return new Network[] {ACTIVE_NETWORK};
    }

    public Network getActiveNetwork() {
        return activeNetworkHandle();
    }

    public NetworkInfo getNetworkInfo(Network network) {
        return isKnownNetwork(network) ? activeNetwork : null;
    }

    public NetworkInfo getNetworkInfo(int networkType) {
        return networkType == TYPE_WIFI ? activeNetwork : null;
    }

    public NetworkCapabilities getNetworkCapabilities(Network network) {
        return isKnownNetwork(network) ? new NetworkCapabilities(false) : null;
    }

    /**
     * Returns the link-layer properties for the process-local default network.
     * Unknown or stale Network handles follow Android's null contract.
     */
    public LinkProperties getLinkProperties(Network network) {
        if (!isKnownNetwork(network)) return null;
        // Return a fresh object so callers cannot mutate service state.
        LinkProperties copy = new LinkProperties();
        copy.setInterfaceName(ACTIVE_LINK_PROPERTIES.getInterfaceName());
        copy.setDomains(ACTIVE_LINK_PROPERTIES.getDomains());
        copy.setDnsServers(new ArrayList<>(ACTIVE_LINK_PROPERTIES.getDnsServers()));
        return copy;
    }

    public void registerDefaultNetworkCallback(NetworkCallback callback) {
        if (callback != null) callback.onAvailable(activeNetworkHandle());
    }

    public void registerDefaultNetworkCallback(NetworkCallback callback, Handler handler) {
        if (callback == null) return;
        Runnable available = () -> callback.onAvailable(activeNetworkHandle());
        if (handler == null) available.run(); else handler.post(available);
    }

    public void registerNetworkCallback(NetworkRequest request, NetworkCallback callback) {
        registerNetworkCallback(request, callback, null);
    }

    public void registerNetworkCallback(
            NetworkRequest request, NetworkCallback callback, Handler handler) {
        if (request == null) throw new NullPointerException("request");
        if (callback == null) throw new NullPointerException("callback");
        Network network = activeNetworkHandle();
        NetworkCapabilities capabilities = getNetworkCapabilities(network);
        if (!request.canBeSatisfiedBy(capabilities)) return;
        Runnable available = () -> {
            callback.onAvailable(network);
            callback.onCapabilitiesChanged(network, capabilities);
            callback.onLinkPropertiesChanged(network, getLinkProperties(network));
        };
        if (handler == null) available.run(); else handler.post(available);
    }

    public void unregisterNetworkCallback(NetworkCallback callback) {}

    /**
     * Request the profile's default network using the Android API-29 callback
     * contract. The Darwin profile has one always-available host network, so a
     * request follows the same immediate callback path as registration.
     */
    public void requestNetwork(NetworkRequest request, NetworkCallback callback) {
        registerNetworkCallback(request, callback);
    }

    public void requestNetwork(
            NetworkRequest request, NetworkCallback callback, Handler handler) {
        registerNetworkCallback(request, callback, handler);
    }

    public void requestNetwork(
            NetworkRequest request, NetworkCallback callback, int timeoutMs) {
        registerNetworkCallback(request, callback);
    }

    public void requestNetwork(
            NetworkRequest request, NetworkCallback callback, int timeoutMs, Handler handler) {
        registerNetworkCallback(request, callback, handler);
    }

    public void requestNetwork(
            NetworkRequest request, NetworkCallback callback, int timeoutMs, Executor executor) {
        if (executor == null) throw new NullPointerException("executor");
        if (request == null) throw new NullPointerException("request");
        if (callback == null) throw new NullPointerException("callback");
        Network network = activeNetworkHandle();
        NetworkCapabilities capabilities = getNetworkCapabilities(network);
        if (!request.canBeSatisfiedBy(capabilities)) return;
        executor.execute(() -> {
            callback.onAvailable(network);
            callback.onCapabilitiesChanged(network, capabilities);
            callback.onLinkPropertiesChanged(network, getLinkProperties(network));
        });
    }

    public boolean isDefaultNetworkActive() {
        return true;
    }

    public void addDefaultNetworkActiveListener(OnNetworkActiveListener listener) {
        if (listener == null) throw new IllegalArgumentException("listener is null");
        networkActiveListeners.addIfAbsent(listener);
    }

    public void removeDefaultNetworkActiveListener(OnNetworkActiveListener listener) {
        if (listener == null) throw new IllegalArgumentException("listener is null");
        networkActiveListeners.remove(listener);
    }

    private Network activeNetworkHandle() { return ACTIVE_NETWORK; }

    private boolean isKnownNetwork(Network network) {
        return network != null && ACTIVE_NETWORK.equals(network);
    }

    /** Listener notified when the system default network has active traffic. */
    public interface OnNetworkActiveListener {
        void onNetworkActive();
    }

    /** Android-compatible callback surface used by connectivity-aware apps. */
    public static class NetworkCallback {
        public NetworkCallback() {}
        public NetworkCallback(int flags) {}
        public void onAvailable(Network network) {}
        public void onLosing(Network network, int maxMsToLive) {}
        public void onLost(Network network) {}
        public void onUnavailable() {}
        public void onCapabilitiesChanged(Network network,
                NetworkCapabilities capabilities) {}
        public void onLinkPropertiesChanged(Network network, LinkProperties linkProperties) {}
        public void onNetworkSuspended(Network network) {}
        public void onNetworkResumed(Network network) {}
        public void onBlockedStatusChanged(Network network, boolean blocked) {}
    }
}
