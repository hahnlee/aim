package dev.darwinart.runtime.am;

import android.content.Intent;

/** Narrow AMS-internal contract for broadcasts sent by other Android system services. */
public interface SystemBroadcasts {
    /** Sends {@code intent} as the system to every user; a sticky intent is retained. */
    void broadcastAsSystem(Intent intent, boolean sticky);
}
