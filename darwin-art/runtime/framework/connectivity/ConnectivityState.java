package dev.darwinart.runtime.connectivity;

/** Read-only connectivity facts supplied by the eventual Darwin host provider. */
public interface ConnectivityState {
    interface Listener {
        void onConnectivityChanged(ConnectivitySnapshot snapshot);
    }

    /** Returns whether the active network is metered, conservatively true when none exists. */
    boolean isActiveNetworkMetered();

    /** Returns one immutable host snapshot for the Android projection. */
    default ConnectivitySnapshot snapshot() {
        return ConnectivitySnapshot.fromMetered(isActiveNetworkMetered());
    }

    /**
     * The HTTP proxy of the active network (LinkProperties.getHttpProxy), or
     * null when the network uses none.
     */
    default android.net.ProxyInfo activeNetworkProxy() {
        return null;
    }

    /** Registers for system-owned state publication when the provider is observable. */
    default void addListener(Listener listener) {}

    /** Removes a listener previously registered through {@link #addListener}. */
    default void removeListener(Listener listener) {}
}
