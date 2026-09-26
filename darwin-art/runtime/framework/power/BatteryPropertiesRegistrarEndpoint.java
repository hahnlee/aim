package dev.darwinart.runtime.power;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * BatteryService's "batteryproperties" binder (BatteryPropertiesRegistrar),
 * answering BatteryManager.getIntProperty/getLongProperty the way
 * HealthServiceWrapperAidl.getProperty does.
 */
public final class BatteryPropertiesRegistrarEndpoint extends Binder {
    // Android 16 IBatteryPropertiesRegistrar.aidl, verified against the pinned framework.jar.
    public static final String DESCRIPTOR = "android.os.IBatteryPropertiesRegistrar";
    public static final int TRANSACTION_GET_PROPERTY = 1;
    public static final int TRANSACTION_SCHEDULE_UPDATE = 2;
    // BatteryManager.BATTERY_PROPERTY_* handled by the health HAL.
    static final int PROPERTY_CAPACITY = 4;
    static final int PROPERTY_STATUS = 6;
    static final int PROPERTY_LAST = 12; // BATTERY_PROPERTY_PART_STATUS
    // getPropertyInternal: 0 on success, -1 for UnsupportedOperationException.
    static final int RESULT_OK = 0;
    static final int RESULT_UNSUPPORTED = -1;

    private final BatteryHealth health;

    public BatteryPropertiesRegistrarEndpoint(BatteryHealth health) {
        if (health == null) throw new NullPointerException("health");
        this.health = health;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code == TRANSACTION_SCHEDULE_UPDATE) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            health.scheduleUpdate();
            return true;
        }
        if (code != TRANSACTION_GET_PROPERTY || reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        int id = data.readInt();
        data.enforceNoDataAvail();
        // BatteryProperty starts at Long.MIN_VALUE; ids outside the HAL's
        // switch succeed without a value, as in getPropertyInternal.
        long value = Long.MIN_VALUE;
        int result = RESULT_OK;
        BatteryHealth.Snapshot snapshot = health.snapshot();
        if (id == PROPERTY_CAPACITY && snapshot != null) {
            value = snapshot.capacity;
        } else if (id == PROPERTY_STATUS && snapshot != null) {
            value = snapshot.status;
        } else if (id >= 1 && id <= PROPERTY_LAST) {
            // Charge counter, currents, energy and the battery health data
            // are not reported by the host power source.
            result = RESULT_UNSUPPORTED;
        }
        reply.writeNoException();
        reply.writeInt(result);
        // out BatteryProperty: present marker, then mValueLong and mValueString (String8).
        reply.writeInt(1);
        reply.writeLong(value);
        reply.writeInt(-1);
        return true;
    }
}
