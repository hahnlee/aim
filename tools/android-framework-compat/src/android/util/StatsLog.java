package android.util;

/** Optional statsd boundary for the detached Android compatibility runtime. */
public final class StatsLog {
    private StatsLog() {}

    public static void write(StatsEvent event) {
        // A standalone app process has no statsd daemon. Android treats this
        // telemetry as best-effort, so dropping the event preserves app/UI
        // semantics without inventing a host analytics service.
    }
}
