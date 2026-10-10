// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public interface IRemoteCallback extends IInterface {
    void sendResult(Bundle data) throws RemoteException;

    abstract class Stub extends Binder implements IRemoteCallback {
        public static IRemoteCallback asInterface(IBinder binder) { throw new RuntimeException("stub"); }
        public Stub() { throw new RuntimeException("stub"); }
        public IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
