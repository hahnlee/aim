package android.content.pm;
public interface IPackageLoadingProgressCallback extends android.os.IInterface {
    void onPackageLoadingProgressChanged(float progress)throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IPackageLoadingProgressCallback {
        public android.os.IBinder asBinder(){throw new RuntimeException("stub");}
    }
}
