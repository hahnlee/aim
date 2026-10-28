package com.android.server.power.stats;

import android.content.Context;
import android.os.BatteryStats;
import android.os.Handler;
import android.os.Looper;
import android.os.Parcel;
import android.os.health.HealthStatsParceler;
import android.os.health.HealthStatsWriter;
import android.os.health.UidHealthStats;
import com.android.internal.os.Clock;
import com.android.internal.os.CpuScalingPolicyReader;
import com.android.internal.os.MonotonicClock;
import com.android.internal.os.PowerProfile;
import com.android.server.am.HealthStatsBatteryStatsWriter;

/**
 * Read-only projection of the native owner's recorded counters (android-16.0.0_r1).
 * Original Uid supplies the initial values of fields with no producer; original
 * HealthStatsBatteryStatsWriter decides which keys exist, units and rounding.
 * One original owner and one reusable Uid keep their observer lists bounded.
 */
public final class NativeHealthStats extends BatteryStatsImpl {
    private long realtimeUs;
    private long uptimeUs;
    private final FrozenUid uid = new FrozenUid(this);

    public NativeHealthStats(Context context) {
        super(new BatteryStatsConfig.Builder().build(), Clock.SYSTEM_CLOCK,
                new MonotonicClock(0, Clock.SYSTEM_CLOCK), null,
                new Handler(Looper.getMainLooper()), null, null,
                new UserInfoProvider() {
                    @Override protected int[] getUserIds() { return new int[] {0}; }
                }, new PowerProfile(context), new CpuScalingPolicyReader().read(),
                new PowerStatsUidResolver());
    }

    @Override public long computeBatteryRealtime(long now, int which) { return realtimeUs; }
    @Override public long computeBatteryUptime(long now, int which) { return uptimeUs; }
    // No screen-state producer is installed in this owner: original STATE_UNKNOWN is off.
    @Override public long computeBatteryScreenOffRealtime(long now, int which) { return realtimeUs; }
    @Override public long computeBatteryScreenOffUptime(long now, int which) { return uptimeUs; }

    /** Header: battery realtime/uptime (us); then fixed records in requested UID order.
     * Record: uid, exists, followed by camera, flashlight, BLE and seven process
     * state timer pairs (count, us); count -1 means the timer was never created.
     */
    public synchronized HealthStatsParceler[] write(long[] values,
            HealthStatsBatteryStatsWriter writer) {
        final int width = 22;
        if (values == null || values.length < 2 || (values.length - 2) % width != 0) {
            throw new IllegalArgumentException("health snapshot counters");
        }
        realtimeUs = values[0];
        uptimeUs = values[1];
        HealthStatsParceler[] results = new HealthStatsParceler[(values.length - 2) / width];
        for (int i = 0; i < results.length; i++) {
            int offset = 2 + i * width;
            HealthStatsWriter uidWriter = new HealthStatsWriter(UidHealthStats.CONSTANTS);
            if (values[offset + 1] != 0) {
                uid.read(values, offset);
                writer.writeUid(uidWriter, this, uid);
            }
            results[i] = new HealthStatsParceler(uidWriter);
        }
        return results;
    }

    private static final class FrozenUid extends BatteryStatsImpl.Uid {
        private final BatteryStats.Timer[] timers = new BatteryStats.Timer[10];

        FrozenUid(NativeHealthStats stats) {
            super(stats, 0, 0, 0);
            for (int i = 0; i < timers.length; i++) {
                timers[i] = new StopwatchTimer(Clock.SYSTEM_CLOCK, null, 0, null,
                        stats.mOnBatteryTimeBase);
            }
        }

        private final boolean[] present = new boolean[10];

        void read(long[] values, int offset) {
            for (int i = 0; i < timers.length; i++) {
                int at = offset + 2 + i * 2;
                present[i] = values[at] >= 0;
                if (!present[i]) continue;
                StopwatchTimer timer = (StopwatchTimer) timers[i];
                Parcel in = Parcel.obtain();
                try {
                    in.writeLong(values[at + 1]);
                    in.writeInt((int) values[at]);
                    in.setDataPosition(0);
                    timer.readSummaryFromParcelLocked(in);
                } finally { in.recycle(); }
            }
        }

        @Override public BatteryStats.Timer getCameraTurnedOnTimer() { return present[0] ? timers[0] : null; }
        @Override public BatteryStats.Timer getFlashlightTurnedOnTimer() { return present[1] ? timers[1] : null; }
        @Override public BatteryStats.Timer getBluetoothScanTimer() { return present[2] ? timers[2] : null; }
        @Override public BatteryStats.Timer getProcessStateTimer(int state) { return present[3 + state] ? timers[3 + state] : null; }
    }
}
