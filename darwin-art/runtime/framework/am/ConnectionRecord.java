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
    final ServiceConnectionOwner owner;
    boolean admitted;
    boolean pendingAdmission;

    ConnectionRecord(IServiceConnection endpoint, IntentBindRecord intentBinding,
            long bindFlags, int callerUid, ServiceConnectionOwner connectionOwner) {
        connection = endpoint;
        connectionBinder = endpoint.asBinder();
        binding = intentBinding;
        flags = bindFlags;
        callingUid = callerUid;
        if (connectionOwner == null) throw new IllegalArgumentException("Missing connection owner");
        owner = connectionOwner;
    }

}
