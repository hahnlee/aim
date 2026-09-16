package dev.darwinart.runtime.connectivity;

import android.net.Network;

/** Test-only projection seam; scheduler only needs the active Network handle. */
public final class ConnectivityProjection {
    public Network activeNetwork(ConnectivitySnapshot snapshot) {
        return snapshot != null && snapshot.hasActiveNetwork()
                ? Network.fromNetworkHandle(0x00000001cafed00dL) : null;
    }
}
