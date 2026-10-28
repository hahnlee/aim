package android.net.connectivity.android.net;

import android.os.Binder;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;

public interface INetworkActivityListener extends IInterface {
    void onNetworkActive() throws RemoteException;
    abstract class Stub extends Binder implements INetworkActivityListener {
        public static INetworkActivityListener asInterface(IBinder binder) { return null; }
    }
}
