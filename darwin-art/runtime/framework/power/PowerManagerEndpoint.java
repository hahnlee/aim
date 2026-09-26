package dev.darwinart.runtime.power;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * Narrow Android 16 power-manager endpoint for the desktop profile.
 *
 * <p>The endpoint owns only the Android Binder wire contract. Host sleep/wake
 * policy remains outside this Android framework boundary and is supplied by
 * an injected provider; AppKit is not used here.
 */
public final class PowerManagerEndpoint extends Binder {
    // Android 16 IPowerManager.aidl, verified against the pinned framework.jar:
    // descriptor android.os.IPowerManager;
    // isInteractive() = 21;
    // isDisplayInteractive(int displayId) = 22.
    public static final String DESCRIPTOR = "android.os.IPowerManager";
    public static final int TRANSACTION_IS_INTERACTIVE = 21;
    public static final int TRANSACTION_IS_DISPLAY_INTERACTIVE = 22;

    private final PowerStateProvider stateProvider;

    public PowerManagerEndpoint(PowerStateProvider stateProvider) {
        if (stateProvider == null) throw new NullPointerException("stateProvider");
        this.stateProvider = stateProvider;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code != TRANSACTION_IS_INTERACTIVE && code != TRANSACTION_IS_DISPLAY_INTERACTIVE) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return false;
        data.enforceInterface(DESCRIPTOR);
        if (code == TRANSACTION_IS_DISPLAY_INTERACTIVE) {
            int displayId = data.readInt();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeBoolean(stateProvider.isDisplayInteractive(displayId));
            return true;
        }
        data.enforceNoDataAvail();
        reply.writeNoException();
        reply.writeBoolean(stateProvider.isInteractive());
        return true;
    }
}
