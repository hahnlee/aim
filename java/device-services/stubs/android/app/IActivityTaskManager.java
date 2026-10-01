// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.Intent;
import android.content.res.Configuration;
import android.os.Bundle;
import android.os.IBinder;
import android.os.RemoteException;

public interface IActivityTaskManager extends android.os.IInterface {
    int startActivityWithConfig(IApplicationThread caller, String callingPackage, String callingFeatureId, Intent intent, String resolvedType, IBinder resultTo, String resultWho, int requestCode, int startFlags, Configuration newConfig, Bundle options, int userId) throws RemoteException;
    boolean updateConfiguration(Configuration values) throws RemoteException;
}
