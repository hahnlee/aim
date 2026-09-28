package android.app;

import android.content.ComponentName;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;

/** Test-only generated-interface stand-in with an injectable local endpoint. */
public interface IServiceConnection extends IInterface {
    void connected(ComponentName name, IBinder service, boolean dead) throws RemoteException;

    abstract class Stub {
        public static IServiceConnection asInterface(IBinder binder) {
            return binder instanceof IServiceConnection ? (IServiceConnection) binder : null;
        }
    }
}
