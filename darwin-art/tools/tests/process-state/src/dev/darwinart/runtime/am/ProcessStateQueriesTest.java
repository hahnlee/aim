package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.os.Parcel;

/** ProcessList.getMemoryInfo thresholds and the error-state query. */
public final class ProcessStateQueriesTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static ActivityManager.MemoryInfo memoryInfo() {
        Parcel reply = new Parcel();
        ProcessStateQueries.writeMemoryInfo(reply);
        check(reply.noException, "memory info omitted writeNoException");
        return (ActivityManager.MemoryInfo) reply.typed;
    }

    public static void main(String[] args) {
        ActivityManager.MemoryInfo info = memoryInfo();
        // updateOomLevels' high table (KiB) for adj 0, 100, 200, 250, 900, 950.
        long foreground = 73728L * 1024;
        long visible = 92160L * 1024;
        long cached = 147456L * 3 / 2 * 1024;
        check(info.hidden("foregroundAppThreshold") == foreground, "foreground threshold");
        check(info.hidden("visibleAppThreshold") == visible, "visible threshold");
        // SERVICE_ADJ 500 and HOME_APP_ADJ 600 fall in the adj-900 level.
        check(info.hidden("secondaryServerThreshold") == cached, "service threshold");
        check(info.threshold == cached, "home app threshold");
        check(info.hidden("hiddenAppThreshold") == cached, "cached app threshold");
        check(info.totalMem == 16L << 30 && info.advertisedMem == 16L << 30,
                "total/advertised memory");
        check(!info.lowMemory, "8 GiB free reported as low memory");

        android.os.Process.freeMemory = cached / 2;
        check(memoryInfo().lowMemory, "free memory below the home threshold was not low");

        System.out.println("process-state-queries: memory levels PASS");
    }
}
