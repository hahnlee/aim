package dev.darwinart.runtime.connectivity;

/** Process-local owner of the host connectivity fact consumed by the Binder endpoint. */
public final class ConnectivityStateOwner implements ConnectivityState {
    private ConnectivitySnapshot snapshot = ConnectivitySnapshot.unavailable();

    /** Publishes one host-observed active-network snapshot. */
    public synchronized void updateActiveNetwork(boolean active, boolean metered) {
        snapshot = ConnectivitySnapshot.fromNetworkPath(active, metered, false, 0);
    }

    /** Clears the host snapshot without treating absence as unmetered connectivity. */
    public synchronized void clearActiveNetwork() {
        snapshot = ConnectivitySnapshot.unavailable();
    }

    public synchronized void updateSnapshot(ConnectivitySnapshot value) {
        if (value == null) throw new NullPointerException("snapshot");
        snapshot = value;
    }

    @Override
    public synchronized boolean isActiveNetworkMetered() {
        return snapshot.isMetered();
    }

    @Override
    public synchronized ConnectivitySnapshot snapshot() {
        return snapshot;
    }
}
