package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.os.Parcel;
import android.os.Parcelable;
import java.lang.reflect.Field;

/**
 * Read-only AMS process-state queries: memory pressure levels and freezer
 * transaction reports. Processes in an error state belong to {@link AppErrors}.
 */
final class ProcessStateQueries {
    // ProcessList oom_adj values that getMemoryInfo reports thresholds for.
    private static final int FOREGROUND_APP_ADJ = 0;
    private static final int VISIBLE_APP_ADJ = 100;
    private static final int SERVICE_ADJ = 500;
    private static final int HOME_APP_ADJ = 600;
    private static final int CACHED_APP_MIN_ADJ = 900;
    // ProcessList.mOomAdj and the lmkd minfree levels (KiB) that
    // updateOomLevels computes for a 64-bit device with more than 700 MB,
    // where the memory scale saturates at the high table.
    private static final int[] OOM_ADJ = {0, 100, 200, 250, 900, 950};
    private static final long[] OOM_MIN_FREE_KIB = {
            73728, 92160, 110592, 129024, 147456 * 3 / 2, 184320 * 7 / 4};

    private ProcessStateQueries() {}

    /** ProcessList.getMemoryInfo. */
    static void writeMemoryInfo(Parcel reply) {
        ActivityManager.MemoryInfo info = new ActivityManager.MemoryInfo();
        long homeAppMem = memLevel(HOME_APP_ADJ);
        long cachedAppMem = memLevel(CACHED_APP_MIN_ADJ);
        info.advertisedMem = processLong("getAdvertisedMem");
        info.availMem = processLong("getFreeMemory");
        info.totalMem = processLong("getTotalMemory");
        info.threshold = homeAppMem;
        info.lowMemory = info.availMem < homeAppMem + (cachedAppMem - homeAppMem) / 2;
        setHiddenLong(info, "hiddenAppThreshold", cachedAppMem);
        setHiddenLong(info, "secondaryServerThreshold", memLevel(SERVICE_ADJ));
        setHiddenLong(info, "visibleAppThreshold", memLevel(VISIBLE_APP_ADJ));
        setHiddenLong(info, "foregroundAppThreshold", memLevel(FOREGROUND_APP_ADJ));
        reply.writeNoException();
        reply.writeTypedObject(info, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
    }

    /**
     * Oneway frozenBinderTransactionDetected reaches CachedAppOptimizer, which
     * acts only on frozen processes. The app freezer is not enabled here.
     */
    static void frozenBinderTransactionDetected(Parcel data) {
        data.readInt(); // debugPid
        data.readInt(); // code
        data.readInt(); // flags
        data.readInt(); // err
        data.enforceNoDataAvail();
    }

    private static long memLevel(int adj) {
        for (int i = 0; i < OOM_ADJ.length; i++) {
            if (adj <= OOM_ADJ[i]) return OOM_MIN_FREE_KIB[i] * 1024;
        }
        return OOM_MIN_FREE_KIB[OOM_ADJ.length - 1] * 1024;
    }

    /** Hidden android.os.Process memory accessors used by ProcessList. */
    private static long processLong(String name) {
        try {
            return (long) android.os.Process.class.getMethod(name).invoke(null);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Process." + name, error);
        }
    }

    private static void setHiddenLong(ActivityManager.MemoryInfo info, String name, long value) {
        try {
            Field field = ActivityManager.MemoryInfo.class.getDeclaredField(name);
            field.setAccessible(true);
            field.setLong(info, value);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("ActivityManager.MemoryInfo." + name, error);
        }
    }
}
