package android.app;

import android.content.Intent;

/** Test-only stand-in for the hidden Android 16 ServiceStartArgs. */
public class ServiceStartArgs {
    public final boolean taskRemoved;
    public final int startId;
    public final int flags;
    public final Intent args;

    public ServiceStartArgs(boolean taskRemoved, int startId, int flags, Intent args) {
        this.taskRemoved = taskRemoved;
        this.startId = startId;
        this.flags = flags;
        this.args = args;
    }
}
