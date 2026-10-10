package android.os.incremental;
public interface IStorageHealthListener extends android.os.IInterface {
    void onHealthStatus(int id,int status)throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IStorageHealthListener {
        public android.os.IBinder asBinder(){throw new RuntimeException("stub");}
    }
}
