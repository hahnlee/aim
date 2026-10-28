package dev.aim.server;

/** Cold Wi-Fi ownership in aim-system; called only by the system UID (#2261). */
interface IWifiBridge {
    void addWifiStateChangedListener(IBinder listener);
    void removeWifiStateChangedListener(IBinder listener);
    byte[] getPasspointConfigurations();
    oneway void onWifiStateChanged(int state);
}
