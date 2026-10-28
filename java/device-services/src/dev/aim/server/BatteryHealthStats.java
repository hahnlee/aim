package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.Bundle;
import android.os.Parcel;
import android.os.health.HealthStatsParceler;
import com.android.server.am.HealthStatsBatteryStatsWriter;
import com.android.server.power.stats.NativeHealthStats;

/** The cold health-read path; all framework mapping and wire encoding stay original. */
final class BatteryHealthStats extends IBatteryHealthStats.Stub {
    private final Context context;
    private NativeHealthStats stats;

    BatteryHealthStats(Context context) { this.context = context; }

    private synchronized NativeHealthStats stats() {
        if (stats == null) {
            // SystemServer loads this original JNI library before BatteryStatsImpl.
            System.loadLibrary("android_servers");
            stats = new NativeHealthStats(context);
        }
        return stats;
    }

    @Override public byte[] writeSnapshot(long[] counters, int format) {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            HealthStatsParceler[] results = stats().write(counters,
                    new HealthStatsBatteryStatsWriter());
            switch (format) {
                case SINGLE -> {
                    if (results.length != 1) throw new IllegalArgumentException("one uid required");
                    out.writeTypedObject(results[0], 0);
                }
                case ARRAY -> out.writeTypedArray(results, 0);
                case RESULT -> {
                    Bundle data = new Bundle(1);
                    data.putParcelableArray("uid_snapshots", results);
                    out.writeInt(0); // IBatteryStats.RESULT_OK
                    out.writeInt(1); // IResultReceiver.send's Bundle presence
                    data.writeToParcel(out, 0);
                }
                default -> throw new IllegalArgumentException("health snapshot format");
            }
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(identity);
        }
    }
}
