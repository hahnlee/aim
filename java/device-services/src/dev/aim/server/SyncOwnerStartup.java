package dev.aim.server;

import android.accounts.AccountManagerInternal;
import android.content.Context;
import com.android.server.LocalServices;
import com.android.server.content.PrivateContentService;

/** Called once, after native local owners are published. Failure requires process restart. */
final class SyncOwnerStartup {
    private final ISystemProcess host;
    private final long generation;
    private final java.util.concurrent.CountDownLatch ready = new java.util.concurrent.CountDownLatch(1);
    private PrivateContentService content;
    private boolean started, booted, unlocked, replaying;
    private SyncOwnerStartup(ISystemProcess host, long generation) {
        this.host = host;
        this.generation = generation;
    }
    void onBootPhase(int phase) {
        content.onBootPhase(phase);
        if (phase == com.android.server.SystemService.PHASE_BOOT_COMPLETED) booted = true;
        ready();
    }
    void onStartUser(int user) {
        content.onStartUser(user);
        if (user == 0) started = true;
        ready();
    }
    private void ready() {
        if (!started || !booted) return;
        // A restart replays before the main looper can deliver any queued receiver.
        if (unlocked) replay();
        ready.countDown();
    }
    private void onUserUnlocking(int userId) {
        if (userId != 0) return;
        try { ready.await(); }
        catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException("system lifecycle interrupted", e);
        }
        replay();
    }
    private synchronized void replay() {
        if (replaying) return;
        replaying = true;
        content.onUnlockUser(0);
        content.replaySyncState(0, () -> {
            try { host.completeSyncOwnerReplay(generation); }
            catch (android.os.RemoteException e) { throw e.rethrowFromSystemServer(); }
        });
    }
    static SyncOwnerStartup start(Context context, ISystemProcess host) throws android.os.RemoteException {
        var accounts = java.util.Objects.requireNonNull(host.getSyncAccounts());
        LocalServices.addService(AccountManagerInternal.class,
                new NativeAccountManagerInternal(context, accounts));
        long generation = host.getSyncOwnerGeneration();
        var startup = new SyncOwnerStartup(host, generation);
        // Subscribe before constructing the original manager and its receivers. A concurrent
        // native unlock waits for startup, then for lifecycle delivery, before broadcasting.
        startup.unlocked = host.attachSystemUserLifecycle(generation, new ISystemUserLifecycle.Stub() {
            @Override public void onUserUnlocking(int userId) { startup.onUserUnlocking(userId); }
            @Override public void onShutdown() {
                Bridge.enforceSystemUid();
                java.util.Objects.requireNonNull(startup.content, "sync construction incomplete").flush();
            }
        });
        var content = new PrivateContentService(context, new NativeSyncAccounts(accounts));
        startup.content = content;
        host.attachSyncOwner(generation, content);
        if (!host.isSyncOwnerAttached(generation))
            throw new IllegalStateException("private sync owner died during construction (#1460)");
        host.attachSyncAdapterQueries(new SyncAdapterQueries(host, content::getSyncAdapterTypes));
        return startup;
    }
}
