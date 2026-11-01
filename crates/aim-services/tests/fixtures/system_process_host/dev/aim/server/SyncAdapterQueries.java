package dev.aim.server;
final class SyncAdapterQueries extends android.os.Binder {
    SyncAdapterQueries(ISystemProcess host, java.util.function.IntFunction<android.content.SyncAdapterType[]> types) {
        if (types == null || host == null) throw new AssertionError("missing sync adapter dependency");
    }
}
