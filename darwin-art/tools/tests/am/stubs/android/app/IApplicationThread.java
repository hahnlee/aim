package android.app;

import android.content.Intent;
import android.content.pm.ParceledListSlice;
import android.content.pm.ServiceInfo;
import android.content.res.CompatibilityInfo;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;

/** Test-only generated-interface stand-in with an injectable local endpoint. */
public interface IApplicationThread extends IInterface {
    void scheduleCreateService(IBinder token, ServiceInfo info, CompatibilityInfo compatInfo,
            int processState) throws RemoteException;
    void scheduleBindService(IBinder token, Intent intent, boolean rebind, int processState,
            long bindSeq) throws RemoteException;
    void scheduleUnbindService(IBinder token, Intent intent) throws RemoteException;
    void scheduleServiceArgs(IBinder token, ParceledListSlice args) throws RemoteException;
    void scheduleStopService(IBinder token) throws RemoteException;
    void scheduleExit() throws RemoteException;

    abstract class Stub {
        public static IApplicationThread asInterface(IBinder binder) {
            return binder instanceof IApplicationThread ? (IApplicationThread) binder : null;
        }
    }
}
