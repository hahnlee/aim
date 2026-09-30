// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.location;

import android.os.IBinder;
import android.os.RemoteException;

public interface ILocationManager extends android.os.IInterface {
    boolean isProviderEnabledForUser(String provider, int userId) throws RemoteException;
    LocationTime getGnssTimeMillis() throws RemoteException;

    abstract class Stub extends android.os.Binder implements ILocationManager {
        public static ILocationManager asInterface(IBinder obj) { throw new RuntimeException("stub"); }
    }
}
