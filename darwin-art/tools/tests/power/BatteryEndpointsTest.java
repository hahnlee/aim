package dev.darwinart.runtime.power;

import android.os.Binder;
import android.os.Parcel;

public final class BatteryEndpointsTest {
    private static final class FakeHealth implements BatteryHealth {
        Snapshot snapshot;
        int updates;
        @Override public Snapshot snapshot() { return snapshot; }
        @Override public void scheduleUpdate() { updates++; }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static Parcel property(BatteryPropertiesRegistrarEndpoint endpoint, int id)
            throws Exception {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        data.writeInterfaceToken(BatteryPropertiesRegistrarEndpoint.DESCRIPTOR);
        data.writeInt(id);
        check(endpoint.onTransact(BatteryPropertiesRegistrarEndpoint.TRANSACTION_GET_PROPERTY,
                data, reply, 0), "getProperty rejected");
        check(reply.hasNoException(), "getProperty omitted writeNoException");
        return reply;
    }

    /** Reads the result code, then the out BatteryProperty value. */
    private static long[] result(Parcel reply) {
        int status = reply.readInt();
        check(reply.readInt() == 1, "out BatteryProperty missing");
        long value = reply.readLong();
        check(reply.readInt() == -1, "BatteryProperty string must be null");
        return new long[] {status, value};
    }

    private static Parcel stats(BatteryStatsEndpoint endpoint, int code) throws Exception {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        data.writeInterfaceToken(BatteryStatsEndpoint.DESCRIPTOR);
        check(endpoint.onTransact(code, data, reply, 0), "batterystats rejected " + code);
        check(reply.hasNoException(), "batterystats omitted writeNoException");
        return reply;
    }

    public static void main(String[] args) throws Exception {
        FakeHealth health = new FakeHealth();
        BatteryPropertiesRegistrarEndpoint properties =
                new BatteryPropertiesRegistrarEndpoint(health);
        BatteryStatsEndpoint stats = new BatteryStatsEndpoint(health);

        // Without a host reading the capacity is unsupported, not zero.
        long[] early = result(property(properties,
                BatteryPropertiesRegistrarEndpoint.PROPERTY_CAPACITY));
        check(early[0] == -1 && early[1] == Long.MIN_VALUE, "capacity before first reading");
        check(!stats(stats, BatteryStatsEndpoint.TRANSACTION_IS_CHARGING).readBoolean(),
                "no reading must not report charging");

        health.snapshot = new BatteryHealth.Snapshot(57, BatteryStatsEndpoint.STATUS_CHARGING);
        long[] capacity = result(property(properties,
                BatteryPropertiesRegistrarEndpoint.PROPERTY_CAPACITY));
        check(capacity[0] == 0 && capacity[1] == 57, "capacity not from reading");
        long[] status = result(property(properties,
                BatteryPropertiesRegistrarEndpoint.PROPERTY_STATUS));
        check(status[0] == 0 && status[1] == BatteryStatsEndpoint.STATUS_CHARGING,
                "status not from reading");
        for (int unsupported : new int[] {1, 2, 3, 5, 7, 12}) {
            long[] value = result(property(properties, unsupported));
            check(value[0] == -1 && value[1] == Long.MIN_VALUE,
                    "property " + unsupported + " must be unsupported");
        }
        long[] unknown = result(property(properties, 99));
        check(unknown[0] == 0 && unknown[1] == Long.MIN_VALUE, "ids outside the HAL switch");

        check(stats(stats, BatteryStatsEndpoint.TRANSACTION_IS_CHARGING).readBoolean(),
                "charging status not reported");
        check(stats(stats, BatteryStatsEndpoint.TRANSACTION_COMPUTE_CHARGE_TIME_REMAINING)
                .readLong() == -1, "charge time must be unknown");
        check(stats(stats, BatteryStatsEndpoint.TRANSACTION_COMPUTE_BATTERY_TIME_REMAINING)
                .readLong() == -1, "battery time must be unknown");
        health.snapshot = new BatteryHealth.Snapshot(100, 5); // BATTERY_STATUS_FULL
        check(!stats(stats, BatteryStatsEndpoint.TRANSACTION_IS_CHARGING).readBoolean(),
                "full battery is not charging");

        Parcel update = Parcel.obtain();
        update.writeInterfaceToken(BatteryPropertiesRegistrarEndpoint.DESCRIPTOR);
        check(properties.onTransact(BatteryPropertiesRegistrarEndpoint.TRANSACTION_SCHEDULE_UPDATE,
                update, null, Binder.FLAG_ONEWAY), "scheduleUpdate rejected");
        check(health.updates == 1, "scheduleUpdate not forwarded");

        Parcel wrong = Parcel.obtain();
        wrong.writeInterfaceToken("not.android.os.IBatteryPropertiesRegistrar");
        wrong.writeInt(BatteryPropertiesRegistrarEndpoint.PROPERTY_CAPACITY);
        boolean rejected = false;
        try {
            properties.onTransact(BatteryPropertiesRegistrarEndpoint.TRANSACTION_GET_PROPERTY,
                    wrong, Parcel.obtain(), 0);
        } catch (SecurityException expected) {
            rejected = true;
        }
        check(rejected, "wrong interface token accepted");
        System.out.println("battery-endpoints: PASS");
    }
}
