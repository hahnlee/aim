package android.content.pm;
public interface IDataLoaderStatusListener extends android.os.IInterface {
    void onStatusChanged(int id,int status)throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IDataLoaderStatusListener {
        public android.os.IBinder asBinder(){throw new RuntimeException("stub");}
    }
}
