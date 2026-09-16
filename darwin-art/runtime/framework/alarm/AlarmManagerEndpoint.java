package dev.darwinart.runtime.alarm;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.os.WorkSource;

/** System-server endpoint for Android's IAlarmManager Binder contract. */
public final class AlarmManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.app.IAlarmManager";
    private static final int TRANSACTION_SET = IBinder.FIRST_CALL_TRANSACTION;
    private static final int TRANSACTION_SET_TIME = IBinder.FIRST_CALL_TRANSACTION + 1;
    private static final int TRANSACTION_SET_TIME_ZONE = IBinder.FIRST_CALL_TRANSACTION + 2;
    private static final int TRANSACTION_REMOVE = IBinder.FIRST_CALL_TRANSACTION + 3;
    private static final int TRANSACTION_REMOVE_ALL = IBinder.FIRST_CALL_TRANSACTION + 4;
    private static final int TRANSACTION_GET_NEXT_WAKE_FROM_IDLE =
            IBinder.FIRST_CALL_TRANSACTION + 5;
    private static final int TRANSACTION_GET_NEXT_ALARM_CLOCK =
            IBinder.FIRST_CALL_TRANSACTION + 6;
    private static final int TRANSACTION_CAN_SCHEDULE_EXACT =
            IBinder.FIRST_CALL_TRANSACTION + 7;
    private static final int TRANSACTION_HAS_SCHEDULE_EXACT =
            IBinder.FIRST_CALL_TRANSACTION + 8;
    private static final int TRANSACTION_GET_CONFIG_VERSION =
            IBinder.FIRST_CALL_TRANSACTION + 9;

    private AlarmManager.AlarmClockInfo nextAlarmClock;
    private PendingIntent nextAlarmOperation;

    public AlarmManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected synchronized boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code < TRANSACTION_SET || code > TRANSACTION_GET_CONFIG_VERSION || reply == null) {
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        switch (code) {
            case TRANSACTION_SET:
                data.readString();
                data.readInt();
                data.readLong();
                data.readLong();
                data.readLong();
                data.readInt();
                PendingIntent operation = data.readTypedObject(PendingIntent.CREATOR);
                data.readStrongBinder();
                data.readString();
                data.readTypedObject(WorkSource.CREATOR);
                AlarmManager.AlarmClockInfo alarmClock =
                        data.readTypedObject(AlarmManager.AlarmClockInfo.CREATOR);
                data.enforceNoDataAvail();
                if (alarmClock != null) {
                    nextAlarmClock = alarmClock;
                    nextAlarmOperation = operation;
                }
                reply.writeNoException();
                return true;
            case TRANSACTION_SET_TIME:
                data.readLong();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeBoolean(false);
                return true;
            case TRANSACTION_SET_TIME_ZONE:
                data.readString();
                data.enforceNoDataAvail();
                reply.writeNoException();
                return true;
            case TRANSACTION_REMOVE:
                PendingIntent removed = data.readTypedObject(PendingIntent.CREATOR);
                data.readStrongBinder();
                data.enforceNoDataAvail();
                if (removed == null || removed.equals(nextAlarmOperation)) clearNextAlarm();
                reply.writeNoException();
                return true;
            case TRANSACTION_REMOVE_ALL:
                data.readString();
                data.enforceNoDataAvail();
                clearNextAlarm();
                reply.writeNoException();
                return true;
            case TRANSACTION_GET_NEXT_WAKE_FROM_IDLE:
                reply.writeNoException();
                reply.writeLong(0L);
                return true;
            case TRANSACTION_GET_NEXT_ALARM_CLOCK:
                data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(nextAlarmClock, 1);
                return true;
            case TRANSACTION_CAN_SCHEDULE_EXACT:
                data.readString();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeBoolean(true);
                return true;
            case TRANSACTION_HAS_SCHEDULE_EXACT:
                data.readString();
                data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeBoolean(true);
                return true;
            case TRANSACTION_GET_CONFIG_VERSION:
                reply.writeNoException();
                reply.writeInt(1);
                return true;
            default:
                return false;
        }
    }

    private void clearNextAlarm() {
        nextAlarmClock = null;
        nextAlarmOperation = null;
    }
}
