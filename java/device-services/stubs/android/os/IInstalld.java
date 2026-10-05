// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public interface IInstalld extends IInterface {
    void reconcileSdkData(ReconcileSdkDataArgs args) throws RemoteException;
    void rmPackageDir(String packageName, String packageDir) throws RemoteException;
    void destroyAppData(String uuid, String packageName, int userId, int flags,
            long ceDataInode) throws RemoteException;
    abstract class Stub extends Binder implements IInstalld {
        public static IInstalld asInterface(IBinder binder) { throw new RuntimeException("stub"); }
    }
}
