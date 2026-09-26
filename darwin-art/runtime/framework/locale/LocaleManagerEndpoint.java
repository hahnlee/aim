package dev.darwinart.runtime.locale;

import android.content.res.Resources;
import android.os.Binder;
import android.os.IBinder;
import android.os.LocaleList;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;

/** Android 16 application-locale endpoint owned by the locale subsystem. */
public final class LocaleManagerEndpoint extends Binder {
    // Android 16 ILocaleManager, pinned to the shipped framework.jar. Only the
    // read query is needed by the current runtime; setters and other queries
    // remain unsupported until their Android-owned state is persisted.
    public static final String DESCRIPTOR = "android.app.ILocaleManager";
    public static final int TRANSACTION_SET_APPLICATION_LOCALES =
            IBinder.FIRST_CALL_TRANSACTION;
    public static final int TRANSACTION_GET_APPLICATION_LOCALES =
            IBinder.FIRST_CALL_TRANSACTION + 1;
    public static final int TRANSACTION_GET_SYSTEM_LOCALES =
            IBinder.FIRST_CALL_TRANSACTION + 2;
    public static final int TRANSACTION_SET_OVERRIDE_LOCALE_CONFIG =
            IBinder.FIRST_CALL_TRANSACTION + 3;
    public static final int TRANSACTION_GET_OVERRIDE_LOCALE_CONFIG =
            IBinder.FIRST_CALL_TRANSACTION + 4;

    public LocaleManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if ((code != TRANSACTION_GET_APPLICATION_LOCALES
                        && code != TRANSACTION_GET_SYSTEM_LOCALES)
                || reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        LocaleList locales;
        if (code == TRANSACTION_GET_APPLICATION_LOCALES) {
            data.readString(); // packageName; no persisted per-app override exists yet.
            data.readInt(); // userId; this profile exposes one Android user.
            // Empty means no per-app override. Resources/Configuration remains
            // the owner of the effective locale selection.
            locales = LocaleList.getEmptyLocaleList();
        } else {
            // Match LocaleManagerService's Android-owned source of truth. The
            // host locale may influence Configuration only through the
            // platform integration boundary, never through this Binder shim.
            locales = Resources.getSystem().getConfiguration().getLocales();
        }
        data.enforceNoDataAvail();

        reply.writeNoException();
        reply.writeTypedObject(locales,
                Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        return true;
    }
}
