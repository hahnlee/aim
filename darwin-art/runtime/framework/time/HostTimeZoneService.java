package dev.darwinart.runtime.time;

import android.content.Intent;
import android.os.SystemProperties;
import android.util.Log;
import dev.darwinart.runtime.am.SystemBroadcasts;
import java.util.TimeZone;

/**
 * The host is this runtime's time zone authority, in the role Android's time
 * zone detector plays: at start and whenever macOS posts a time zone change,
 * its zone becomes the Android default the way AlarmManagerService
 * setTimeZoneImpl applies one.
 */
public final class HostTimeZoneService {
    private static final String TAG = "DarwinHostTimeZone";
    private static final String TIMEZONE_PROPERTY = "persist.sys.timezone";

    private final SystemBroadcasts broadcasts;

    public HostTimeZoneService(SystemBroadcasts broadcasts) {
        if (broadcasts == null) throw new NullPointerException("broadcasts");
        this.broadcasts = broadcasts;
    }

    /** Applies the current host zone, then follows host changes. */
    public void start() {
        synchronize();
        Thread watcher = new Thread(() -> {
            while (nativeAwaitHostTimeZoneChange()) synchronize();
            Log.w(TAG, "host time zone notifications unavailable");
        }, "DarwinHostTimeZone");
        watcher.setDaemon(true);
        watcher.start();
    }

    private synchronized void synchronize() {
        String zone = nativeHostTimeZone();
        if (zone == null) {
            Log.w(TAG, "host time zone is not readable");
            return;
        }
        setTimeZone(zone);
    }

    /** AlarmManagerService.setTimeZoneImpl for a zone the host selected. */
    private void setTimeZone(String zone) {
        TimeZone timeZone = TimeZone.getTimeZone(zone);
        if (!zone.equals(timeZone.getID())) {
            // Not in this image's tzdata; the unknown-zone fallback is GMT.
            Log.w(TAG, "host time zone " + zone + " is not in tzdata");
            return;
        }
        boolean changed = !zone.equals(SystemProperties.get(TIMEZONE_PROPERTY));
        if (changed) {
            Log.i(TAG, "time zone " + zone);
            SystemProperties.set(TIMEZONE_PROPERTY, zone);
        }
        // Clear the cached default: the property now names the zone.
        TimeZone.setDefault(null);
        if (changed) {
            Intent intent = new Intent(Intent.ACTION_TIMEZONE_CHANGED);
            intent.addFlags(Intent.FLAG_RECEIVER_REPLACE_PENDING
                    | Intent.FLAG_RECEIVER_FOREGROUND
                    | Intent.FLAG_RECEIVER_INCLUDE_BACKGROUND);
            intent.putExtra(Intent.EXTRA_TIMEZONE, zone);
            broadcasts.broadcastAsSystem(intent, false);
        }
    }

    private static native String nativeHostTimeZone();
    private static native boolean nativeAwaitHostTimeZoneChange();
}
