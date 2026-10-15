// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public interface IInstalld extends IInterface {
    void clearAppData(String uuid, String packageName, int userId, int flags, long ceDataInode) throws RemoteException;
    void linkNativeLibraryDirectory(String uuid, String packageName, String nativeLibPath32, int userId) throws RemoteException;
    void reconcileSdkData(ReconcileSdkDataArgs args) throws RemoteException;
    void rmPackageDir(String packageName, String packageDir) throws RemoteException;
    void destroyAppData(String uuid, String packageName, int userId, int flags,
            long ceDataInode) throws RemoteException;
    abstract class Stub extends Binder implements IInstalld {
        public static IInstalld asInterface(IBinder binder) { throw new RuntimeException("stub"); }
    }
 int FLAG_STORAGE_DE=1,FLAG_STORAGE_CE=2;
 CreateAppDataResult createAppData(CreateAppDataArgs args)throws RemoteException;
 int FLAG_STORAGE_EXTERNAL=4,FLAG_CLEAR_CODE_CACHE_ONLY=32;
 long[] getAppSize(String volume,String[] names,int user,int flags,int appId,long[] ceInodes,String[] paths)throws RemoteException;
 void moveCompleteApp(String from,String to,String name,int appId,String seInfo,int target,String source)throws RemoteException;
 void destroyAppProfiles(String name)throws RemoteException;
 int FLAG_CLEAR_CACHE_ONLY=16;
}
