package android.net.wifi;
public interface IWifiStateChangedListener extends android.os.IInterface {
    void onWifiStateChanged() throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IWifiStateChangedListener {
        public static IWifiStateChangedListener asInterface(android.os.IBinder binder) {
            throw new RuntimeException("stub");
        }
    }
}
