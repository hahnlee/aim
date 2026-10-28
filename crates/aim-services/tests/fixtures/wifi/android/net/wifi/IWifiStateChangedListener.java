package android.net.wifi;
import android.os.*;
public interface IWifiStateChangedListener extends IInterface {
    void onWifiStateChanged() throws RemoteException;
    abstract class Stub extends Binder implements IWifiStateChangedListener {
        public IBinder asBinder() { return this; }
        public static IWifiStateChangedListener asInterface(IBinder binder) {
            if (binder == null) return null;
            // A new proxy per call proves unregister uses Binder identity.
            IWifiStateChangedListener target = (IWifiStateChangedListener) binder;
            return new IWifiStateChangedListener() {
                public IBinder asBinder() { return binder; }
                public void onWifiStateChanged() throws RemoteException { target.onWifiStateChanged(); }
            };
        }
    }
}
