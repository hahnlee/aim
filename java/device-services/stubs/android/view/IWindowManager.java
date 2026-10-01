// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.view;

import android.os.IBinder;
import android.os.RemoteException;

public interface IWindowManager extends android.os.IInterface {
    void setWindowingMode(int displayId, int mode) throws RemoteException;

    abstract class Stub extends android.os.Binder implements IWindowManager {
        public static IWindowManager asInterface(IBinder obj) { throw new RuntimeException("stub"); }
    }
}
