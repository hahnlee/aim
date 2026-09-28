package dev.aim.runtime.connectivity;

/** Android permission boundary used by the connectivity Binder owner. */
public interface ConnectivityPermissionEnforcer {
    void enforceAccessNetworkState();
}
