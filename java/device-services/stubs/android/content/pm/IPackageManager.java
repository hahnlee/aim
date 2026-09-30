// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import android.os.RemoteException;

public interface IPackageManager extends android.os.IInterface {
    boolean isFirstBoot() throws RemoteException;
    boolean isDeviceUpgrading() throws RemoteException;
}
