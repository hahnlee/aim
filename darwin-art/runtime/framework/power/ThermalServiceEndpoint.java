package dev.darwinart.runtime.power;

import android.os.Binder;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Parcel;
import android.os.RemoteException;
import java.util.ArrayList;

/**
 * Narrow Android 16 thermal-service endpoint used while constructing PowerManager.
 *
 * <p>The desktop profile has no Android thermal sensor authority. Read-only
 * queries therefore report Android empty/unavailable values. Status listeners
 * follow ThermalManagerService semantics: registration is tracked until
 * explicit removal or Binder death, and the current status is posted after a
 * successful registration.
 */
public final class ThermalServiceEndpoint extends Binder {
    // Android 16 IThermalService.aidl, verified against the pinned framework.jar.
    public static final String DESCRIPTOR = "android.os.IThermalService";
    public static final int TRANSACTION_REGISTER_THERMAL_EVENT_LISTENER = 1;
    public static final int TRANSACTION_REGISTER_THERMAL_EVENT_LISTENER_WITH_TYPE = 2;
    public static final int TRANSACTION_UNREGISTER_THERMAL_EVENT_LISTENER = 3;
    public static final int TRANSACTION_GET_CURRENT_TEMPERATURES = 4;
    public static final int TRANSACTION_GET_CURRENT_TEMPERATURES_WITH_TYPE = 5;
    public static final int TRANSACTION_REGISTER_THERMAL_STATUS_LISTENER = 6;
    public static final int TRANSACTION_UNREGISTER_THERMAL_STATUS_LISTENER = 7;
    public static final int TRANSACTION_GET_CURRENT_THERMAL_STATUS = 8;
    public static final int TRANSACTION_GET_CURRENT_COOLING_DEVICES = 9;
    public static final int TRANSACTION_GET_CURRENT_COOLING_DEVICES_WITH_TYPE = 10;
    public static final int TRANSACTION_GET_THERMAL_HEADROOM = 11;
    public static final int TRANSACTION_GET_THERMAL_HEADROOM_THRESHOLDS = 12;
    public static final int TRANSACTION_REGISTER_THERMAL_HEADROOM_LISTENER = 13;
    public static final int TRANSACTION_UNREGISTER_THERMAL_HEADROOM_LISTENER = 14;
    private static final String STATUS_LISTENER_DESCRIPTOR =
            "android.os.IThermalStatusListener";
    private static final int TRANSACTION_STATUS_CHANGED = IBinder.FIRST_CALL_TRANSACTION;

    private final Handler callbackHandler;
    private final ArrayList<StatusListenerRecord> statusListeners = new ArrayList<>();

    public ThermalServiceEndpoint() {
        this(new Handler(Looper.getMainLooper()));
    }

    ThermalServiceEndpoint(Handler callbackHandler) {
        if (callbackHandler == null) {
            throw new IllegalArgumentException("thermal callback handler is null");
        }
        this.callbackHandler = callbackHandler;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }

        // Event and headroom listeners need real sensor authorities and remain
        // unsupported. Status listeners are valid even with THERMAL_STATUS_NONE.
        if (code != TRANSACTION_GET_CURRENT_TEMPERATURES
                && code != TRANSACTION_GET_CURRENT_TEMPERATURES_WITH_TYPE
                && code != TRANSACTION_REGISTER_THERMAL_STATUS_LISTENER
                && code != TRANSACTION_UNREGISTER_THERMAL_STATUS_LISTENER
                && code != TRANSACTION_GET_CURRENT_THERMAL_STATUS
                && code != TRANSACTION_GET_CURRENT_COOLING_DEVICES
                && code != TRANSACTION_GET_CURRENT_COOLING_DEVICES_WITH_TYPE
                && code != TRANSACTION_GET_THERMAL_HEADROOM
                && code != TRANSACTION_GET_THERMAL_HEADROOM_THRESHOLDS) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return false;
        data.enforceInterface(DESCRIPTOR);
        switch (code) {
            case TRANSACTION_REGISTER_THERMAL_STATUS_LISTENER: {
                IBinder listener = data.readStrongBinder();
                data.enforceNoDataAvail();
                boolean registered = registerStatusListener(listener);
                reply.writeNoException();
                reply.writeBoolean(registered);
                return true;
            }
            case TRANSACTION_UNREGISTER_THERMAL_STATUS_LISTENER: {
                IBinder listener = data.readStrongBinder();
                data.enforceNoDataAvail();
                boolean unregistered = unregisterStatusListener(listener);
                reply.writeNoException();
                reply.writeBoolean(unregistered);
                return true;
            }
            case TRANSACTION_GET_CURRENT_TEMPERATURES:
            case TRANSACTION_GET_CURRENT_COOLING_DEVICES:
                data.enforceNoDataAvail();
                reply.writeNoException();
                // AIDL typed-array empty encoding; there are no desktop sensors.
                reply.writeInt(0);
                return true;
            case TRANSACTION_GET_CURRENT_TEMPERATURES_WITH_TYPE:
            case TRANSACTION_GET_CURRENT_COOLING_DEVICES_WITH_TYPE:
                data.readInt(); // type
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeInt(0);
                return true;
            case TRANSACTION_GET_CURRENT_THERMAL_STATUS:
                data.enforceNoDataAvail();
                reply.writeNoException();
                // android.os.PowerManager.THERMAL_STATUS_NONE.
                reply.writeInt(0);
                return true;
            case TRANSACTION_GET_THERMAL_HEADROOM:
                data.readInt(); // forecastSeconds
                data.enforceNoDataAvail();
                reply.writeNoException();
                // No forecast is available without an Android thermal source.
                reply.writeFloat(Float.NaN);
                return true;
            case TRANSACTION_GET_THERMAL_HEADROOM_THRESHOLDS:
                data.enforceNoDataAvail();
                reply.writeNoException();
                // PowerManager indexes statuses 1..6; NaN means no threshold.
                reply.writeInt(7);
                for (int status = 0; status < 7; status++) {
                    reply.writeFloat(Float.NaN);
                }
                return true;
            default:
                return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
    }

    private boolean registerStatusListener(IBinder listener) throws RemoteException {
        if (listener == null) return false;
        final StatusListenerRecord record;
        synchronized (statusListeners) {
            for (StatusListenerRecord existing : statusListeners) {
                if (existing.listener == listener) return false;
            }
            record = new StatusListenerRecord(listener);
            listener.linkToDeath(record, 0);
            statusListeners.add(record);
        }
        // AOSP ThermalManagerService posts the current status through FgThread
        // after registration. The system main looper is this profile's matching
        // serialized Android service callback authority.
        callbackHandler.post(new Runnable() {
            @Override
            public void run() {
                dispatchCurrentStatus(record);
            }
        });
        return true;
    }

    private boolean unregisterStatusListener(IBinder listener) {
        if (listener == null) return false;
        synchronized (statusListeners) {
            for (int index = statusListeners.size() - 1; index >= 0; --index) {
                StatusListenerRecord record = statusListeners.get(index);
                if (record.listener == listener) {
                    statusListeners.remove(index);
                    listener.unlinkToDeath(record, 0);
                    return true;
                }
            }
        }
        return false;
    }

    private void dispatchCurrentStatus(StatusListenerRecord record) {
        synchronized (statusListeners) {
            if (!statusListeners.contains(record)) return;
        }
        Parcel callback = Parcel.obtain();
        try {
            callback.writeInterfaceToken(STATUS_LISTENER_DESCRIPTOR);
            callback.writeInt(0); // PowerManager.THERMAL_STATUS_NONE.
            record.listener.transact(
                    TRANSACTION_STATUS_CHANGED, callback, null, IBinder.FLAG_ONEWAY);
        } catch (RemoteException dead) {
            record.binderDied();
        } finally {
            callback.recycle();
        }
    }

    private final class StatusListenerRecord implements IBinder.DeathRecipient {
        final IBinder listener;

        StatusListenerRecord(IBinder listener) {
            this.listener = listener;
        }

        @Override
        public void binderDied() {
            unregisterStatusListener(listener);
        }
    }
}
