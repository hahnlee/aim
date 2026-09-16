package dev.darwinart.runtime.uimode;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Android 16 IUiModeManager state exposed by system_server. */
public final class UiModeManagerEndpoint extends Binder {
    public static final String DESCRIPTOR = "android.app.IUiModeManager";
    public static final int TRANSACTION_GET_CURRENT_MODE_TYPE =
            IBinder.FIRST_CALL_TRANSACTION + 4;
    public static final int TRANSACTION_GET_NIGHT_MODE =
            IBinder.FIRST_CALL_TRANSACTION + 6;
    public static final int TRANSACTION_IS_UI_MODE_LOCKED =
            IBinder.FIRST_CALL_TRANSACTION + 12;
    public static final int TRANSACTION_IS_NIGHT_MODE_LOCKED =
            IBinder.FIRST_CALL_TRANSACTION + 13;
    public static final int TRANSACTION_GET_CONTRAST =
            IBinder.FIRST_CALL_TRANSACTION + 26;
    public static final int TRANSACTION_GET_FORCE_INVERT_STATE =
            IBinder.FIRST_CALL_TRANSACTION + 27;

    private static final int UI_MODE_TYPE_NORMAL = 1;
    private static final int MODE_NIGHT_NO = 1;

    public UiModeManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code >= IBinder.FIRST_CALL_TRANSACTION && code <= IBinder.LAST_CALL_TRANSACTION) {
            data.enforceInterface(DESCRIPTOR);
        }
        switch (code) {
            case TRANSACTION_GET_CURRENT_MODE_TYPE:
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeInt(UI_MODE_TYPE_NORMAL);
                return true;
            case TRANSACTION_GET_NIGHT_MODE:
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeInt(MODE_NIGHT_NO);
                return true;
            case TRANSACTION_IS_UI_MODE_LOCKED:
            case TRANSACTION_IS_NIGHT_MODE_LOCKED:
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeBoolean(false);
                return true;
            case TRANSACTION_GET_CONTRAST:
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeFloat(0.0f);
                return true;
            case TRANSACTION_GET_FORCE_INVERT_STATE:
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeInt(0);
                return true;
            default:
                return super.onTransact(code, data, reply, flags);
        }
    }
}
