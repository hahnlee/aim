package dev.aim.server;
final class SyncAdapterQueries extends android.os.Binder {
    SyncAdapterQueries(android.content.Context context, ISystemProcess host) {
        if (context == null || host == null) throw new AssertionError("missing sync adapter dependency");
    }
}
