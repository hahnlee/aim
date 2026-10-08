// Compile-only pinned image API; never packaged as runtime implementation.
package android.content.pm;
public interface IPackageManagerNative extends android.os.IInterface {
 void registerStagedApexObserver(IStagedApexObserver observer)throws android.os.RemoteException;
 abstract class Stub extends android.os.Binder implements IPackageManagerNative {public static IPackageManagerNative asInterface(android.os.IBinder binder){throw new RuntimeException("stub");}}
}
