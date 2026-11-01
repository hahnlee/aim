package android.os;
public interface IRemoteCallback extends IInterface {
    void sendResult(Bundle result) throws RemoteException;
    abstract class Stub extends Binder implements IRemoteCallback {
        public IBinder asBinder() { return this; }
        public static IRemoteCallback asInterface(IBinder binder) { return (IRemoteCallback) binder; }
    }
}
