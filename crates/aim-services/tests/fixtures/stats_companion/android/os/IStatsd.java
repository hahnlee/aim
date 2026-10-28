// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public interface IStatsd {
    IBinder asBinder();
    void informAllUidData(ParcelFileDescriptor fd) throws RemoteException;
    void informOnePackage(String name,int uid,long version,String versionName,
        String installer,byte[] hash) throws RemoteException;
    void informOnePackageRemoved(String name,int uid) throws RemoteException;
    void informDeviceShutdown() throws RemoteException;
    void updateProperties(android.util.PropertyParcel[] properties) throws RemoteException;
    abstract class Stub { public static IStatsd asInterface(IBinder binder) { return (IStatsd)binder; } }
}
