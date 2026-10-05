// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.RemoteException;

public interface IPackageManager extends android.os.IInterface {
    ApplicationInfo getApplicationInfo(String packageName, long flags, int userId) throws RemoteException;
    PackageInfo getPackageInfo(String packageName, long flags, int userId) throws RemoteException;
    String getSdkSandboxPackageName() throws RemoteException;
    boolean isFirstBoot() throws RemoteException;
    boolean isDeviceUpgrading() throws RemoteException;
    void registerPackageMonitorCallback(android.os.IRemoteCallback callback, int userId) throws RemoteException;
    String getSuspendingPackage(String packageName, int userId) throws RemoteException;
    int[] getPackageGids(String packageName, long flags, int userId) throws RemoteException;
    InstallSourceInfo getInstallSourceInfo(String packageName, int userId) throws RemoteException;
    byte[] getPreferredActivityBackup(int userId) throws RemoteException;
    android.content.ComponentName getInstantAppResolverComponent() throws RemoteException;
    android.content.ComponentName getInstantAppInstallerComponent() throws RemoteException;
    abstract class Stub extends android.os.Binder implements IPackageManager {
        public static IPackageManager asInterface(android.os.IBinder binder) { throw new RuntimeException("stub"); }
    }
}
