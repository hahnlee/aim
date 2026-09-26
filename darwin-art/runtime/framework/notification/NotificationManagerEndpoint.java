package dev.darwinart.runtime.notification;

import android.app.Notification;
import android.app.NotificationChannel;
import android.content.pm.ParceledListSlice;
import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;

/** System-process owner for the pinned Android 16 INotificationManager contract. */
public final class NotificationManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.app.INotificationManager";
    private final int enqueueNotificationWithTagCode = transaction("enqueueNotificationWithTag");
    private final int cancelNotificationWithTagCode = transaction("cancelNotificationWithTag");
    private final int cancelAllNotificationsCode = transaction("cancelAllNotifications");
    private final int getAppActiveNotificationsCode = transaction("getAppActiveNotifications");
    private final int createNotificationChannelsCode = transaction("createNotificationChannels");
    private final int getNotificationChannelCode = transaction("getNotificationChannel");
    private final int getNotificationChannelsCode = transaction("getNotificationChannels");
    private final int deleteNotificationChannelCode = transaction("deleteNotificationChannel");
    private final int getZenModeCode = transaction("getZenMode");
    private final NotificationRecords records = new NotificationRecords();

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
        final int uid = Binder.getCallingUid();
        if (code == enqueueNotificationWithTagCode) {
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            data.readString(); // operation package
            String tag = data.readString();
            int id = data.readInt();
            Notification notification = data.readTypedObject(Notification.CREATOR);
            int userId = data.readInt();
            data.enforceNoDataAvail();
            records.enqueue(uid, Binder.getCallingPid(), packageName, tag, id, notification,
                    userId);
            reply.writeNoException();
            return true;
        }
        if (code == cancelNotificationWithTagCode) {
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            data.readString(); // operation package
            String tag = data.readString();
            int id = data.readInt();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            records.cancel(uid, packageName, tag, id, userId);
            reply.writeNoException();
            return true;
        }
        if (code == cancelAllNotificationsCode) {
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            records.cancelAll(uid, packageName, userId);
            reply.writeNoException();
            return true;
        }
        if (code == getAppActiveNotificationsCode) {
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeTypedObject(new ParceledListSlice<>(records.active(uid, packageName,
                    userId)), Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == createNotificationChannelsCode) {
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            ParceledListSlice<?> channels = data.readTypedObject(ParceledListSlice.CREATOR);
            data.enforceNoDataAvail();
            if (channels == null) throw new NullPointerException("channelsList");
            records.createChannels(uid, packageName, channels.getList());
            reply.writeNoException();
            return true;
        }
        if (code == getNotificationChannelCode) {
            data.enforceInterface(DESCRIPTOR);
            data.readString(); // calling package
            data.readInt(); // user id
            String packageName = data.readString();
            String channelId = data.readString();
            data.enforceNoDataAvail();
            NotificationChannel channel = records.channel(uid, packageName, channelId);
            reply.writeNoException();
            reply.writeTypedObject(channel, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getNotificationChannelsCode) {
            data.enforceInterface(DESCRIPTOR);
            data.readString(); // calling package
            String packageName = data.readString();
            data.readInt(); // user id
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeTypedObject(new ParceledListSlice<>(records.channels(uid, packageName)),
                    Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getZenModeCode) {
            // ZenModeHelper.getZenMode: Do Not Disturb is never entered here
            // (no host Focus provider yet), so the mode is ZEN_MODE_OFF.
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeInt(android.provider.Settings.Global.ZEN_MODE_OFF);
            return true;
        }
        if (code == deleteNotificationChannelCode) {
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            String channelId = data.readString();
            data.enforceNoDataAvail();
            records.deleteChannel(uid, packageName, channelId);
            reply.writeNoException();
            return true;
        }
        return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
    }
}
