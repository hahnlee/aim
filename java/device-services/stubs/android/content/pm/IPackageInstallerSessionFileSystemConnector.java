package android.content.pm;
public interface IPackageInstallerSessionFileSystemConnector extends android.os.IInterface {
    void writeData(String name,long offset,long length,android.os.ParcelFileDescriptor fd)throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IPackageInstallerSessionFileSystemConnector {
        public android.os.IBinder asBinder(){throw new RuntimeException("stub");}
    }
}
