package dev.darwinart.runtime.os;

import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Client for the legacy getService transaction retained by Android 16 AIDL. */
public final class SystemServices {
    private SystemServices() {}
    private static IBinder manager;
    private static native IBinder nativeConnect();
    public static synchronized IBinder getService(String name) throws RemoteException {
        if (manager == null) manager = nativeConnect();
        if (manager == null) throw new RemoteException("System service manager unavailable");
        return lookup(manager, name);
    }
    public static IBinder lookup(IBinder manager, String name) throws RemoteException {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken("android.os.IServiceManager");
            data.writeString(name);
            if (!manager.transact(IBinder.FIRST_CALL_TRANSACTION, data, reply, 0)) {
                throw new RemoteException("System service lookup rejected");
            }
            reply.readException();
            return reply.readStrongBinder();
        } finally {
            reply.recycle();
            data.recycle();
        }
    }
}
