package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.Handler;
import com.android.server.FgThread;
import java.util.Locale;
import java.util.TimeZone;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

public final class PackageBootstrapJavaTest {
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) throws Exception {
        var bridge = new PackageBootstrapBridge();
        for (int uid : new int[] {0, 2000, 10100}) {
            Binder.uid = uid;
            for (Runnable action : new Runnable[] {
                    () -> bridge.onExistingPackageInstalled(null, 0, 0),
                    () -> bridge.resetPreferredRuntimePermissions(0),
                    () -> bridge.getPreferredCrossProfileAccessControl(0, 10),
                    () -> bridge.formatPackageTimestamp(0),
                    () -> bridge.waitPackageBackgroundHandler(100),
                    () -> bridge.sendPreferredActivityChanged(0)}) {
                try { action.run(); throw new AssertionError("non-system caller accepted"); }
                catch (SecurityException expected) {}
            }
        }
        Binder.uid = 1000;
        ExistingInstallJavaTest.run(args[0]);
        PackageLocalOwnersTest.run();
        Locale.setDefault(Locale.US);
        TimeZone.setDefault(TimeZone.getTimeZone("UTC"));
        check(bridge.formatPackageTimestamp(0).equals("1970-01-01 00:00:00"));
        check(bridge.formatPackageTimestamp(-1).equals("1969-12-31 23:59:59"));
        TimeZone.setDefault(TimeZone.getTimeZone("Asia/Seoul"));
        check(bridge.formatPackageTimestamp(0).equals("1970-01-01 09:00:00"));
        Locale.setDefault(Locale.forLanguageTag("th-TH"));
        check(bridge.formatPackageTimestamp(0).equals("2513-01-01 09:00:00"));

        for (int user : new int[] {-1, 0, 10}) bridge.sendPreferredActivityChanged(user);
        check(Context.broadcasts.isEmpty());
        while (!FgThread.pending.isEmpty()) FgThread.pending.remove().run();
        check(Context.broadcasts.size() == 3);
        for (int index = 0; index < 3; index++) {
            var sent = Context.broadcasts.get(index);
            int user = new int[] {-1, 0, 10}[index];
            check(sent.user() == user && sent.intent().user == user);
            check(sent.intent().action.equals("android.intent.action.ACTION_PREFERRED_ACTIVITY_CHANGED"));
            check(sent.intent().extra.equals("android.intent.extra.user_handle"));
            check(sent.intent().flags == 0x04000000);
        }
        android.app.ActivityManager.available = false;
        bridge.sendPreferredActivityChanged(0);
        FgThread.pending.remove().run();
        check(Context.broadcasts.size() == 3);

        check(bridge.waitPackageBackgroundHandler(0)); // Already dispatched barrier.
        Handler.dispatch = action -> {}; // Blocked worker: timeout is a false result.
        check(!bridge.waitPackageBackgroundHandler(1));
        check(!bridge.waitPackageBackgroundHandler(-1));
        var posted = new CountDownLatch(1);
        var done = new CountDownLatch(1);
        Runnable[] barrier = new Runnable[1];
        Handler.dispatch = action -> { barrier[0] = action; posted.countDown(); };
        Thread waiter = new Thread(() -> {
            check(bridge.waitPackageBackgroundHandler(5000));
            done.countDown();
        });
        waiter.start();
        try {
            check(posted.await(1, TimeUnit.SECONDS));
            check(done.getCount() == 1);
            waiter.interrupt(); // NativePackageHandlers retries interruptions.
            barrier[0].run();
            check(done.await(1, TimeUnit.SECONDS));
        } finally { waiter.join(6000); }
        check(!waiter.isAlive());
    }
}
