// Compile-only pinned image API; checked by the device-services build.
package android.os;
public interface IVold extends IInterface {
    void fixupAppDir(String path, int uid) throws RemoteException;
    abstract class Stub extends Binder implements IVold {
        public static IVold asInterface(IBinder binder) { throw new RuntimeException("stub"); }
    }
}
