package dev.darwinart.runtime.trust;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * Narrow Android 16 trust-manager endpoint for the desktop profile.
 *
 * <p>The host has no Android lock-screen or credential state. Consequently
 * both queries deliberately report the conservative unsecured/unlocked state;
 * macOS Touch ID integration does not belong on this Android framework
 * boundary.
 */
public final class TrustManagerEndpoint extends Binder {
    // Android 16 ITrustManager.aidl, verified against the local framework.jar:
    // descriptor android.app.trust.ITrustManager;
    // isDeviceLocked(int userId, int deviceId) = 10;
    // isDeviceSecure(int userId, int deviceId) = 11.
    public static final String DESCRIPTOR = "android.app.trust.ITrustManager";
    public static final int TRANSACTION_IS_DEVICE_LOCKED = 10;
    public static final int TRANSACTION_IS_DEVICE_SECURE = 11;

    public TrustManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code != TRANSACTION_IS_DEVICE_LOCKED && code != TRANSACTION_IS_DEVICE_SECURE) {
            return super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return false;
        data.enforceInterface(DESCRIPTOR);
        data.readInt(); // userId; this profile has no credential state for any user.
        data.readInt(); // deviceId; the desktop profile has no lock state.
        data.enforceNoDataAvail();
        reply.writeNoException();
        reply.writeBoolean(false);
        return true;
    }
}
