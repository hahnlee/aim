// Compile-only pinned image API; never packaged as runtime implementation.
package android.os.storage;
public interface IStorageManager extends android.os.IInterface {
 long lastMaintenance()throws android.os.RemoteException;
 void runMaintenance()throws android.os.RemoteException;
 boolean supportsCheckpoint()throws android.os.RemoteException;
 void startCheckpoint(int retries)throws android.os.RemoteException;
}
