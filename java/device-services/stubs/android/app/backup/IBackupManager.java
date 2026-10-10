// Compile-only pinned image API; no implementation shipped.
package android.app.backup;
public interface IBackupManager extends android.os.IInterface {
 boolean isUserReadyForBackup(int userId) throws android.os.RemoteException;
 void restoreAtInstallForUser(int userId,String packageName,int token) throws android.os.RemoteException;
 abstract class Stub extends android.os.Binder implements IBackupManager {
  public static IBackupManager asInterface(android.os.IBinder binder) {throw new RuntimeException("stub");}
 }
}
