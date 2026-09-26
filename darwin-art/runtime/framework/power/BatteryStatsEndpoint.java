package dev.darwinart.runtime.power;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * The "batterystats" binder queries BatteryManager makes: charging state and
 * the time-remaining estimates. No battery history is kept, so both estimates
 * are unknown (-1), as BatteryStatsImpl reports without enough history.
 */
public final class BatteryStatsEndpoint extends Binder {
    // Android 16 IBatteryStats.aidl, verified against the pinned framework.jar.
    public static final String DESCRIPTOR = "com.android.internal.app.IBatteryStats";
    public static final int TRANSACTION_IS_CHARGING = 17;
    public static final int TRANSACTION_COMPUTE_BATTERY_TIME_REMAINING = 18;
    public static final int TRANSACTION_COMPUTE_CHARGE_TIME_REMAINING = 19;
    // BatteryManager.BATTERY_STATUS_CHARGING.
    static final int STATUS_CHARGING = 2;

    private final BatteryHealth health;

    public BatteryStatsEndpoint(BatteryHealth health) {
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
        if (reply == null || (code != TRANSACTION_IS_CHARGING
                && code != TRANSACTION_COMPUTE_BATTERY_TIME_REMAINING
                && code != TRANSACTION_COMPUTE_CHARGE_TIME_REMAINING)) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        data.enforceNoDataAvail();
        reply.writeNoException();
        if (code == TRANSACTION_IS_CHARGING) {
            BatteryHealth.Snapshot snapshot = health.snapshot();
            reply.writeBoolean(snapshot != null && snapshot.status == STATUS_CHARGING);
        } else {
            reply.writeLong(-1);
        }
        return true;
    }
}
