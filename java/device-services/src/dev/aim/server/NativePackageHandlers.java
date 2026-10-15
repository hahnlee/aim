package dev.aim.server;

import com.android.internal.os.BackgroundThread;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Barrier on the original background handler used by retained package helpers. */
public final class NativePackageHandlers {
    private NativePackageHandlers() {}
    public static boolean waitForBackground(long timeoutMillis) {
        CountDownLatch latch = new CountDownLatch(1);
        BackgroundThread.getHandler().post(latch::countDown);
        long end = System.currentTimeMillis() + timeoutMillis;
        while (latch.getCount() > 0) {
            try {
                long remaining = end - System.currentTimeMillis();
                if (remaining <= 0) return false;
                return latch.await(remaining, TimeUnit.MILLISECONDS);
            } catch (InterruptedException retry) { }
        }
        return true;
    }
}
