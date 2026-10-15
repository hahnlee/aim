package android.content.pm;
public interface IDataLoader extends android.os.IInterface {
    void create(int id,DataLoaderParamsParcel params,android.content.pm.FileSystemControlParcel control,IDataLoaderStatusListener listener)throws android.os.RemoteException;
    void start(int id)throws android.os.RemoteException;
    void prepareImage(int id,InstallationFileParcel[] added,String[] removed)throws android.os.RemoteException;
    void destroy(int id)throws android.os.RemoteException;
}
