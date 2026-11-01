package dev.aim.server;

import android.content.Context;
import android.content.Intent;
import java.util.ArrayDeque;

/** Production coordinator and capability; Android handler/account boundaries are doubles. */
public final class SyncOwnerLifecycleTest {
    public static boolean unlocked, ready, alive = true, fail;
    static int accounts;
    private static int observed, replays;
    static SyncReplayOriginal original;
    static final ArrayDeque<Runnable> handler = new ArrayDeque<>();
    public static void unlockFlag(int user) { original.onUnlockUser(user); }
    public static void replay(int user, Runnable complete) {
        require(user == 0);
        require(SystemProcessHostTest.events.get(SystemProcessHostTest.events.size()-1).equals("sync:unlock=0"));
        require(original.mUnlockedUsers.getOrDefault(0, false));
        original.replay(user, complete);
        replays++;
    }
    static void observe(int count) { observed = count; }
    private static void require(boolean condition) { if (!condition) throw new AssertionError(); }
    private static void boot(SyncOwnerStartup sync) {
        for (int phase : new int[] {480, 500, 520, 550, 600, 1000}) sync.onBootPhase(phase);
    }
    private static void unlock(Context context, int user) {
        context.unlockReceiver.onReceive(context, new Intent(Intent.ACTION_USER_UNLOCKED)
                .putExtra(Intent.EXTRA_USER_HANDLE, user));
    }
    static void run(Context context, ISystemProcess host) throws Exception {
        for (int scenario = 0; scenario < 6; scenario++) {
            unlocked = scenario == 0; // Missed unlock, before registration.
            ready = fail = false;
            alive = true;
            accounts = 1; observed = replays = 0; handler.clear();
            original = new SyncReplayOriginal();
            var sync = SyncOwnerStartup.start(context, host);
            original.noUser(context);
            require(handler.isEmpty());
            if (scenario == 1) { unlocked = true; unlock(context, 0); } // Early unlock.
            sync.onStartUser(0);
            require(!ready && handler.isEmpty());
            if (scenario >= 2) {
                boot(sync);
                unlock(context, android.os.UserHandle.USER_NULL);
                unlock(context, 10);
                require(handler.isEmpty()); // No-user and other-user broadcasts never unlock 0.
                unlocked = true;
                fail = scenario == 2;
                try { unlock(context, 0); require(!fail); }
                catch (IllegalStateException e) { require(fail && e.getMessage().contains("snapshot")); }
            } else boot(sync);
            require(!ready);
            if (fail) {
                fail = false;
                while (!handler.isEmpty()) handler.remove().run();
                require(!ready && replays == 0); continue;
            }
            require(replays == 1);
            // Commit an account change while the replay's refresh is still queued.
            accounts = 2;
            fail = scenario == 3;
            handler.remove().run(); // Original onUnlockUser handler log.
            try { handler.remove().run(); require(!fail); }
            catch (IllegalStateException e) { require(fail && !ready); handler.clear(); continue; }
            require(observed == 2 && !ready); // Refresh is insufficient: barrier must run.
            if (scenario == 5) alive = false; // Death between refresh and completion.
            try { handler.remove().run(); require(alive && ready); }
            catch (IllegalStateException e) { require(!alive && !ready); }
            if (alive) { unlock(context, 0); require(handler.isEmpty() && replays == 1); }
        }
    }
}
