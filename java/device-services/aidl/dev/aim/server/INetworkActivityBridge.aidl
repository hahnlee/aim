package dev.aim.server;

/** LegacyNetworkActivityTracker's Ethernet listener ownership in aim-system (#1509). */
interface INetworkActivityBridge {
    void registerNetworkActivityListener(IBinder listener);
    void unregisterNetworkActivityListener(IBinder listener);
    boolean isDefaultNetworkActive();
    oneway void onDefaultNetworkChanged(boolean available);
}
