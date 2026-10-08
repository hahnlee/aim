// Compile-only pinned original AIDL API.
package android.content.pm;

public interface IPackageMoveObserver extends android.os.IInterface {
    void onCreated(int moveId, android.os.Bundle extras) throws android.os.RemoteException;
    void onStatusChanged(int moveId, int status, long estMillis) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IPackageMoveObserver {
        public static IPackageMoveObserver asInterface(android.os.IBinder binder) { throw new RuntimeException("stub"); }
    }
}
