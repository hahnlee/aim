package dev.darwinart.runtime.notification;

import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;

/** System-process owner for the pinned Android 16 INotificationManager contract. */
public final class NotificationManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.app.INotificationManager";
    private final int cancelNotificationWithTagCode = transaction("cancelNotificationWithTag");
    private final int getNotificationChannelsCode = transaction("getNotificationChannels");

    public NotificationManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field = Class.forName("android.app.INotificationManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != cancelNotificationWithTagCode) {
            if (code != getNotificationChannelsCode) {
                return super.onTransact(code, data, reply, flags);
            }
            data.enforceInterface(DESCRIPTOR);
            data.readString(); // calling package
            data.readString(); // target package
            data.readInt(); // user id
            data.enforceNoDataAvail();
            reply.writeNoException();
            // ParceledListSlice is hidden from the SDK compile jar, but this
            // is the exact Android 16 return shape consumed by the public
            // NotificationManager.getNotificationChannels() wrapper. Resolve
            // its pinned emptyList() factory at runtime rather than emitting
            // a second class with the framework-owned descriptor.
            reply.writeTypedObject(emptyParceledListSlice(), 0);
            return true;
        }
        data.enforceInterface(DESCRIPTOR);
        data.readString(); // packageName
        data.readString(); // operation packageName
        data.readString(); // tag
        data.readInt(); // notification id
        data.readInt(); // user id
        data.enforceNoDataAvail();
        reply.writeNoException();
        return true;
    }

    private static Parcelable emptyParceledListSlice() {
        try {
            return (Parcelable) Class.forName("android.content.pm.ParceledListSlice")
                    .getMethod("emptyList")
                    .invoke(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }
}
