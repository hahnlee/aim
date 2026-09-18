package dev.darwinart.tests;

import android.os.IBinder;
import java.util.concurrent.atomic.AtomicInteger;

/** A real Binder death recipient used by the native/JVM boundary fixture. */
public final class Recipient implements IBinder.DeathRecipient {
    private final AtomicInteger deathCount = new AtomicInteger();

    public Recipient() {}

    @Override public void binderDied() {
        deathCount.incrementAndGet();
    }

    public int deathCount() {
        return deathCount.get();
    }
}
