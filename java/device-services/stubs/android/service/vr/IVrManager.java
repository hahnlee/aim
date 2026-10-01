// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.service.vr;

import android.os.IBinder;
import android.os.RemoteException;

public interface IVrManager extends android.os.IInterface {
    void registerListener(IVrStateCallbacks cb) throws RemoteException;

    abstract class Stub extends android.os.Binder implements IVrManager {
        public static IVrManager asInterface(IBinder obj) { throw new RuntimeException("stub"); }
    }
}
