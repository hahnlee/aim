package dev.aim.server;

import android.accounts.AccountManagerInternal;
import android.content.Context;
import com.android.server.LocalServices;
import com.android.server.content.PrivateContentService;

/** Called once, after native local owners are published. Failure requires process restart. */
final class SyncOwnerStartup {
    private final ISystemProcess host;
    private final long generation;
    private final PrivateContentService content;
    private boolean started, booted, replaying;
    private SyncOwnerStartup(Context context, ISystemProcess host, long generation,
            PrivateContentService content) {
        this.host = host;
        this.generation = generation;
        this.content = content;
        // Subscribe before the native snapshot; USER_UNLOCKED is not sticky.
        context.registerReceiverAsUser(new android.content.BroadcastReceiver() {
            @Override public void onReceive(Context c, android.content.Intent intent) {
                if (android.content.Intent.ACTION_USER_UNLOCKED.equals(intent.getAction())
                        && intent.getIntExtra(android.content.Intent.EXTRA_USER_HANDLE,
                                android.os.UserHandle.USER_NULL) == 0) replay();
            }
        }, android.os.UserHandle.ALL,
                new android.content.IntentFilter(android.content.Intent.ACTION_USER_UNLOCKED),
                null, null);
    }
    void onBootPhase(int phase) {
        content.onBootPhase(phase);
        if (phase == com.android.server.SystemService.PHASE_BOOT_COMPLETED) booted = true;
        replay();
    }
    void onStartUser(int user) {
        content.onStartUser(user);
        if (user == 0) started = true;
        replay();
    }
    private void replay() {
        if (!started || !booted || replaying) return;
        try {
            if (!host.isSyncOwnerUserUnlocked(generation)) return;
            replaying = true;
            content.onUnlockUser(0);
            content.replaySyncState(0, () -> {
                try { host.completeSyncOwnerReplay(generation); }
                catch (android.os.RemoteException e) { throw e.rethrowFromSystemServer(); }
            });
        } catch (android.os.RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    static SyncOwnerStartup start(Context context, ISystemProcess host) throws android.os.RemoteException {
        var accounts = java.util.Objects.requireNonNull(host.getSyncAccounts());
        LocalServices.addService(AccountManagerInternal.class,
                new NativeAccountManagerInternal(context, accounts));
        long generation = host.getSyncOwnerGeneration();
        var content = new PrivateContentService(context, new NativeSyncAccounts(accounts));
        host.attachSyncOwner(generation, content);
        if (!host.isSyncOwnerAttached(generation))
            throw new IllegalStateException("private sync owner died during construction (#1460)");
        host.attachSyncAdapterQueries(new SyncAdapterQueries(host, content::getSyncAdapterTypes));
        return new SyncOwnerStartup(context, host, generation, content);
    }
}
