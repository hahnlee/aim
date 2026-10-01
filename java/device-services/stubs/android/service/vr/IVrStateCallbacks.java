// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.service.vr;

import android.os.RemoteException;

public interface IVrStateCallbacks extends android.os.IInterface {
    void onVrStateChanged(boolean enabled) throws RemoteException;

    abstract class Stub extends android.os.Binder implements IVrStateCallbacks {
        public Stub() { throw new RuntimeException("stub"); }
        public android.os.IBinder asBinder() { throw new RuntimeException("stub"); }
    }
}
