package dev.aim.server;

import android.content.Context;
import java.util.ArrayDeque;

/** Production coordinator and pinned original bodies; handler/account transports are doubles. */
public final class SyncOwnerLifecycleTest {
    public static boolean unlocked, ready, alive = true, fail;
    static ISystemUserLifecycle lifecycle;
    static int accounts;
    private static int observed, replays, acknowledgements;
    static SyncReplayOriginal original;
    static final ArrayDeque<Runnable> handler = new ArrayDeque<>();
    public static void unlockFlag(int user) { original.onUnlockUser(user); }
    public static void replay(int user, Runnable complete) {
        require(user == 0 && original.mUnlockedUsers.getOrDefault(0, false));
        original.replay(user, complete);
        replays++;
    }
    static void observe(int count) { observed = count; }
    static void acknowledge() {
        require(alive && !ready);
        ready = true;
        acknowledgements++;
    }
    private static void require(boolean condition) { if (!condition) throw new AssertionError(); }
    private static void boot(SyncOwnerStartup sync) {
        for (int phase : new int[] {480, 500, 520, 550, 600, 1000}) sync.onBootPhase(phase);
    }
    private static SyncOwnerStartup start(Context context, ISystemProcess host) throws Exception {
        ready = fail = false; lifecycle = null; alive = true;
        accounts = 1; observed = replays = acknowledgements = 0; handler.clear();
        original = new SyncReplayOriginal();
        var sync = SyncOwnerStartup.start(context, host);
        original.noUser(context);
        require(handler.isEmpty());
        sync.onStartUser(0);
        require(!ready && handler.isEmpty());
        return sync;
    }
    private static void drain() { while (!handler.isEmpty()) handler.remove().run(); }
    static void run(Context context, ISystemProcess host) throws Exception {
        // Startup snapshot, including a restart after unlock and a broadcast queued at construction.
        for (int attempt = 0; attempt < 2; attempt++) {
            unlocked = true;
            var sync = start(context, host);
            boot(sync);
            require(replays == 1 && !ready);
            original.broadcast(context, 0); // First possible main-looper receiver execution.
            drain();
            require(ready && acknowledgements == 1);
            lifecycle.onUserUnlocking(0);
            drain();
            require(replays == 1 && acknowledgements == 1);
        }
        // Native late unlock delivers the synchronous callback before dispatch to the original.
        unlocked = false;
        var sync = start(context, host);
        boot(sync);
        require(handler.isEmpty());
        lifecycle.onUserUnlocking(android.os.UserHandle.USER_NULL);
        lifecycle.onUserUnlocking(10);
        require(handler.isEmpty());
        unlocked = true;
        lifecycle.onUserUnlocking(0);
        require(replays == 1 && original.mUnlockedUsers.get(0));
        original.broadcast(context, 0);
        accounts = 2; // Account change before the original handler refresh.
        drain();
        require(ready && observed == 2 && acknowledgements == 1);
        // Unlock during construction/startup blocks native dispatch until phase delivery.
        unlocked = false;
        sync = start(context, host);
        unlocked = true;
        var failure = new java.util.concurrent.atomic.AtomicReference<Throwable>();
        var entered = new java.util.concurrent.CountDownLatch(1);
        Thread nativeUnlock = new Thread(() -> {
            entered.countDown();
            try { lifecycle.onUserUnlocking(0); original.broadcast(context, 0); }
            catch (Throwable error) { failure.set(error); }
        });
        nativeUnlock.start(); entered.await();
        require(!ready && handler.isEmpty());
        boot(sync);
        nativeUnlock.join(5000);
        require(!nativeUnlock.isAlive() && failure.get() == null && replays == 1);
        drain(); require(ready && acknowledgements == 1);
        // Death after delivery but before completion: the old completion cannot acknowledge.
        unlocked = false;
        sync = start(context, host); boot(sync);
        unlocked = true; lifecycle.onUserUnlocking(0);
        handler.remove().run(); handler.remove().run();
        alive = false;
        try { handler.remove().run(); throw new AssertionError(); }
        catch (IllegalStateException expected) { require(!ready); }
        // Replacement sees the native snapshot before any queued broadcast can run.
        sync = start(context, host); boot(sync);
        original.broadcast(context, 0); drain();
        require(ready && acknowledgements == 1);
        // Snapshot/account failure is not converted to readiness.
        unlocked = false;
        sync = start(context, host); boot(sync);
        unlocked = true; fail = true;
        try { lifecycle.onUserUnlocking(0); throw new AssertionError(); }
        catch (IllegalStateException expected) { require(!ready && replays == 0); }
        handler.clear(); fail = false;
    }
}
