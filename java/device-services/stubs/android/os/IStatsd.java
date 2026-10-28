// Compile-only android-16.0.0_r1 StatsD API; checked against the image.
package android.os;
public interface IStatsd extends IInterface {
    void informAllUidData(ParcelFileDescriptor fd) throws RemoteException;
    void informOnePackage(String name, int uid, long version, String versionName,
            String installer, byte[] certificateHash) throws RemoteException;
    void informOnePackageRemoved(String name, int uid) throws RemoteException;
    void informDeviceShutdown() throws RemoteException;
    void updateProperties(android.util.PropertyParcel[] properties) throws RemoteException;
    abstract class Stub extends Binder implements IStatsd {
        public static IStatsd asInterface(IBinder binder) { throw new RuntimeException("stub"); }
    }
}
