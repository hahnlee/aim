package dev.darwinart.runtime.admin;

import android.content.ComponentName;
import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.lang.reflect.Field;
import java.util.Collections;

/** Android 16 device-policy state for an unmanaged single-user device. */
public final class DevicePolicyManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.app.admin.IDevicePolicyManager";
    private static final int PRIMARY_USER = 0;

    private final int getActiveAdminsCode = transaction("getActiveAdmins");
    private final int getDeviceOwnerComponentCode = transaction("getDeviceOwnerComponent");
    private final int getDeviceOwnerComponentOnUserCode =
            transaction("getDeviceOwnerComponentOnUser");
    private final int hasDeviceOwnerCode = transaction("hasDeviceOwner");
    private final int getProfileOwnerAsUserCode = transaction("getProfileOwnerAsUser");

    public DevicePolicyManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.app.admin.IDevicePolicyManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static int requirePrimaryUser(Parcel data) {
        int userId = data.readInt();
        if (userId != PRIMARY_USER) {
            throw new SecurityException("Only Android user 0 is present");
        }
        return userId;
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (reply == null) return false;
        if (code != getActiveAdminsCode && code != getDeviceOwnerComponentCode
                && code != getDeviceOwnerComponentOnUserCode && code != hasDeviceOwnerCode
                && code != getProfileOwnerAsUserCode) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        if (code == getActiveAdminsCode || code == getDeviceOwnerComponentOnUserCode
                || code == getProfileOwnerAsUserCode) {
            requirePrimaryUser(data);
        } else if (code == getDeviceOwnerComponentCode) {
            data.readBoolean(); // callingUserOnly; there is no owner on any user.
        }
        data.enforceNoDataAvail();

        reply.writeNoException();
        if (code == getActiveAdminsCode) {
            reply.writeTypedList(Collections.<ComponentName>emptyList());
        } else if (code == hasDeviceOwnerCode) {
            reply.writeBoolean(false);
        } else {
            reply.writeTypedObject(null, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        }
        return true;
    }
}
