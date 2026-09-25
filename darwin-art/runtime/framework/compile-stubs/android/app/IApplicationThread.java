package android.app;

import android.content.IIntentReceiver;
import android.content.Intent;
import android.content.pm.ParceledListSlice;
import android.content.pm.ServiceInfo;
import android.content.res.CompatibilityInfo;
import android.os.Bundle;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public interface IApplicationThread extends IInterface {
    void scheduleCreateService(IBinder token, ServiceInfo info, CompatibilityInfo compatInfo,
            int processState) throws RemoteException;
    void scheduleBindService(IBinder token, Intent intent, boolean rebind, int processState,
            long bindSeq) throws RemoteException;
    void scheduleUnbindService(IBinder token, Intent intent) throws RemoteException;
    void scheduleServiceArgs(IBinder token, ParceledListSlice args) throws RemoteException;
    void scheduleStopService(IBinder token) throws RemoteException;
    void scheduleExit() throws RemoteException;
    void scheduleRegisteredReceiver(IIntentReceiver receiver, Intent intent, int resultCode,
            String data, Bundle extras, boolean ordered, boolean sticky, boolean assumeDelivered,
            int sendingUser, int processState, int sendingUid, String sendingPackage)
            throws RemoteException;

    abstract class Stub {
        public static IApplicationThread asInterface(IBinder binder) { return null; }
    }
}
