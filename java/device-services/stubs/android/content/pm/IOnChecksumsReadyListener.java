// Compile-only pinned original API; verified by the device-services image linkage gate.
package android.content.pm;
public interface IOnChecksumsReadyListener extends android.os.IInterface {
    void onChecksumsReady(java.util.List<ApkChecksum> checksums) throws android.os.RemoteException;
    abstract class Stub extends android.os.Binder implements IOnChecksumsReadyListener {
        public Stub() { throw new RuntimeException("stub"); }
        public static IOnChecksumsReadyListener asInterface(android.os.IBinder binder) { throw new RuntimeException("stub"); }
        public android.os.IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
