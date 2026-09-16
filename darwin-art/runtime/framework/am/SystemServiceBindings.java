package dev.darwinart.runtime.am;

import android.app.IServiceConnection;
import android.content.Intent;
import android.os.RemoteException;

/** Narrow AMS-internal service binding contract for other Android system services. */
public interface SystemServiceBindings {
    int bindService(Intent intent, IServiceConnection connection) throws RemoteException;

    boolean unbindService(IServiceConnection connection) throws RemoteException;
}
