// Compile-only pinned image API; never packaged as runtime implementation.
package android.content.pm;
public interface IStagedApexObserver extends android.os.IInterface {
 void onApexStaged(ApexStagedEvent event)throws android.os.RemoteException;
 abstract class Stub extends android.os.Binder implements IStagedApexObserver {public android.os.IBinder asBinder(){throw new RuntimeException("stub");}}
}
