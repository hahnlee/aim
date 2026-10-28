package dev.aim.server;
import android.os.*;
interface IWifiBridge {
    void addWifiStateChangedListener(IBinder listener);
    void removeWifiStateChangedListener(IBinder listener);
    byte[] getPasspointConfigurations();
    void onWifiStateChanged(int state);
    abstract class Stub extends Binder implements IWifiBridge {}
}
