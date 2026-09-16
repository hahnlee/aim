package dev.darwinart.runtime.appops;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Android-owned AppOps query endpoint for the desktop profile. */
public final class AppOpsServiceEndpoint extends Binder {
    // Android 16 IAppOpsService, pinned to the shipped framework.jar.
    public static final String DESCRIPTOR = "com.android.internal.app.IAppOpsService";
    public static final int TRANSACTION_CHECK_OPERATION_FOR_DEVICE =
            IBinder.FIRST_CALL_TRANSACTION + 54;
    public static final int MODE_ALLOWED = 0;

    public AppOpsServiceEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code != TRANSACTION_CHECK_OPERATION_FOR_DEVICE || reply == null) {
            return super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        data.readInt(); // operation code; the desktop profile has no restrictions.
        data.readInt(); // uid; package identity is already established by the caller.
        data.readString(); // packageName.
        data.readString(); // attributionTag.
        data.readInt(); // virtualDeviceId.
        data.enforceNoDataAvail();

        reply.writeNoException();
        reply.writeInt(MODE_ALLOWED);
        return true;
    }
}
