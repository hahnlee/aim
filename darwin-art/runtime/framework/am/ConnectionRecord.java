package dev.darwinart.runtime.am;

import android.app.IServiceConnection;
import android.os.IBinder;

/** One caller-owned IServiceConnection registered against an Intent binding. */
final class ConnectionRecord {
    final IServiceConnection connection;
    final IBinder connectionBinder;
    final IntentBindRecord binding;
    final long flags;
    final int callingUid;

    ConnectionRecord(IServiceConnection endpoint, IntentBindRecord intentBinding,
            long bindFlags, int callerUid) {
        connection = endpoint;
        connectionBinder = endpoint.asBinder();
        binding = intentBinding;
        flags = bindFlags;
        callingUid = callerUid;
    }
}
