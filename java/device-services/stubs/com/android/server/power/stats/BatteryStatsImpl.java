package com.android.server.power.stats;
import android.os.BatteryStats;
import com.android.internal.os.*;
public class BatteryStatsImpl extends BatteryStats {
    protected final TimeBase mOnBatteryTimeBase = null;
    public BatteryStatsImpl(BatteryStatsConfig config, Clock clock, MonotonicClock monotonic,
            java.io.File dir, android.os.Handler handler, PlatformIdleStateCallback idle,
            EnergyStatsRetriever energy, UserInfoProvider users, PowerProfile power,
            CpuScalingPolicies policies, PowerStatsUidResolver resolver) { throw new RuntimeException("stub"); }
    public long computeBatteryRealtime(long now, int which) { throw new RuntimeException("stub"); }
    public long computeBatteryUptime(long now, int which) { throw new RuntimeException("stub"); }
    public long computeBatteryScreenOffRealtime(long now, int which) { throw new RuntimeException("stub"); }
    public long computeBatteryScreenOffUptime(long now, int which) { throw new RuntimeException("stub"); }
    public static class BatteryStatsConfig {
        private BatteryStatsConfig(Builder builder) {}
        public static class Builder {
            public Builder() { throw new RuntimeException("stub"); }
            public BatteryStatsConfig build() { throw new RuntimeException("stub"); }
        }
    }
    public interface PlatformIdleStateCallback {}
    public interface EnergyStatsRetriever {}
    public abstract static class UserInfoProvider {
        protected abstract int[] getUserIds();
    }
    public static class TimeBase {}
    public abstract static class Timer extends BatteryStats.Timer {
        public Timer(Clock clock, int type, TimeBase base) { throw new RuntimeException("stub"); }
    }
    public static class StopwatchTimer extends Timer {
        public StopwatchTimer(Clock clock, Uid uid, int type, java.util.ArrayList<StopwatchTimer> pool,
                TimeBase base) { super(clock, type, base); throw new RuntimeException("stub"); }
        public void readSummaryFromParcelLocked(android.os.Parcel in) { throw new RuntimeException("stub"); }
    }
    public static class Uid extends BatteryStats.Uid {
        public Uid(BatteryStatsImpl stats, int uid, long realtime, long uptime) { throw new RuntimeException("stub"); }
        public BatteryStats.Timer getCameraTurnedOnTimer() { throw new RuntimeException("stub"); }
        public BatteryStats.Timer getFlashlightTurnedOnTimer() { throw new RuntimeException("stub"); }
        public BatteryStats.Timer getBluetoothScanTimer() { throw new RuntimeException("stub"); }
        public BatteryStats.Timer getProcessStateTimer(int state) { throw new RuntimeException("stub"); }
    }
}
