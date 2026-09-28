package android.app;

import android.content.ComponentName;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public interface IServiceConnection extends IInterface {
    void connected(ComponentName name, IBinder service, boolean dead) throws RemoteException;

    abstract class Stub {
        public static IServiceConnection asInterface(IBinder binder) { return null; }
    }
}
