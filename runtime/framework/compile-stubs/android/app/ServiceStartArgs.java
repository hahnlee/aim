package android.app;

import android.content.Intent;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public class ServiceStartArgs {
    public final boolean taskRemoved;
    public final int startId;
    public final int flags;
    public final Intent args;

    public ServiceStartArgs(boolean taskRemoved, int startId, int flags, Intent args) {
        throw new RuntimeException("stub");
    }
}
