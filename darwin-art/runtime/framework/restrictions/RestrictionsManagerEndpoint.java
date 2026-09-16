package dev.darwinart.runtime.restrictions;

import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcelable;
import android.os.Parcel;
import android.os.RemoteException;

/** Android-owned restrictions state for the desktop profile. */
public final class RestrictionsManagerEndpoint extends Binder {
    // Android 16 IRestrictionsManager, pinned to the shipped framework.jar.
    public static final String DESCRIPTOR = "android.content.IRestrictionsManager";
    public static final int TRANSACTION_GET_APPLICATION_RESTRICTIONS =
            IBinder.FIRST_CALL_TRANSACTION;
    public static final int TRANSACTION_HAS_RESTRICTIONS_PROVIDER =
            IBinder.FIRST_CALL_TRANSACTION + 2;

    public RestrictionsManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code != TRANSACTION_GET_APPLICATION_RESTRICTIONS
                && code != TRANSACTION_HAS_RESTRICTIONS_PROVIDER) {
            return super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return super.onTransact(code, data, reply, flags);

        data.enforceInterface(DESCRIPTOR);
        if (code == TRANSACTION_GET_APPLICATION_RESTRICTIONS) {
            data.readString(); // packageName; no desktop application restrictions exist.
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeTypedObject(new Bundle(), Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }

        data.enforceNoDataAvail();
        reply.writeNoException();
        // The desktop profile has no device-policy restrictions provider.
        reply.writeBoolean(false);
        return true;
    }
}
