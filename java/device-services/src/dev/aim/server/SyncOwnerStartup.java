package dev.aim.server;

import android.accounts.AccountManagerInternal;
import android.content.Context;
import com.android.server.LocalServices;
import com.android.server.content.PrivateContentService;

/** Called once, after native local owners are published. Failure requires process restart. */
final class SyncOwnerStartup {
    static void start(Context context, ISystemProcess host) throws android.os.RemoteException {
        var accounts = java.util.Objects.requireNonNull(host.getSyncAccounts());
        LocalServices.addService(AccountManagerInternal.class,
                new NativeAccountManagerInternal(context, accounts));
        long generation = host.getSyncOwnerGeneration();
        var content = new PrivateContentService(context, new NativeSyncAccounts(accounts));
        host.attachSyncOwner(generation, content);
        if (!host.isSyncOwnerAttached(generation))
            throw new IllegalStateException("private sync owner died during construction (#1460)");
        host.attachSyncAdapterQueries(new SyncAdapterQueries(host, content::getSyncAdapterTypes));
    }
}
