package dev.darwinart.runtime.power;

import android.content.Intent;
import android.os.BatteryManager;
import dev.darwinart.runtime.am.SystemBroadcasts;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * BatteryService: publishes the sticky ACTION_BATTERY_CHANGED from the host
 * power source and the power-connection and low-battery transitions, and
 * serves the same reading to the batteryproperties and batterystats binders.
 *
 * <p>A host without an internal battery (a desktop Mac) reports a
 * not-present, AC-powered battery at full level, so apps reading level/scale
 * see no low-battery condition.</p>
 */
public final class BatteryService implements BatteryHealth {
    // config_lowBatteryWarningLevel / config_lowBatteryCloseWarningBump defaults.
    static final int LOW_BATTERY_WARNING_LEVEL = 20;
    static final int LOW_BATTERY_CLOSE_WARNING_LEVEL = LOW_BATTERY_WARNING_LEVEL + 5;
    // Host power sources change on the order of seconds; the health HAL's
    // slow periodic chore interval is one minute.
    private static final long POLL_SECONDS = 60;

    /** The values ACTION_BATTERY_CHANGED carries, derived from a reading. */
    static final class State {
        final boolean present;
        final int level;
        final int scale;
        final int status;
        final int health;
        final int plugged;

        State(BatteryStateProvider.Reading reading) {
            present = reading.present;
            level = present ? reading.level : 100;
            scale = present && reading.scale > 0 ? reading.scale : 100;
            plugged = reading.externalPower ? BatteryManager.BATTERY_PLUGGED_AC : 0;
            health = present ? BatteryManager.BATTERY_HEALTH_GOOD
                    : BatteryManager.BATTERY_HEALTH_UNKNOWN;
            if (!present) {
                status = BatteryManager.BATTERY_STATUS_UNKNOWN;
            } else if (reading.charging) {
                status = BatteryManager.BATTERY_STATUS_CHARGING;
            } else if (reading.charged) {
                status = BatteryManager.BATTERY_STATUS_FULL;
            } else if (plugged != 0) {
                status = BatteryManager.BATTERY_STATUS_NOT_CHARGING;
            } else {
                status = BatteryManager.BATTERY_STATUS_DISCHARGING;
            }
        }

        boolean low() {
            return plugged == 0 && status != BatteryManager.BATTERY_STATUS_UNKNOWN
                    && level * 100 / scale <= LOW_BATTERY_WARNING_LEVEL;
        }

        boolean sameAs(State other) {
            return other != null && present == other.present && level == other.level
                    && scale == other.scale && status == other.status
                    && health == other.health && plugged == other.plugged;
        }
    }

    private final BatteryStateProvider provider;
    private final SystemBroadcasts broadcasts;
    private final ScheduledExecutorService poller = Executors.newSingleThreadScheduledExecutor(
            runnable -> {
                Thread thread = new Thread(runnable, "BatteryService");
                thread.setDaemon(true);
                return thread;
            });
    private State last;
    private boolean lowWarned;
    private int sequence;

    public BatteryService(BatteryStateProvider provider, SystemBroadcasts broadcasts) {
        this.provider = provider;
        this.broadcasts = broadcasts;
    }

    public void start() {
        poller.scheduleWithFixedDelay(this::update, 0, POLL_SECONDS, TimeUnit.SECONDS);
    }

    @Override
    public synchronized Snapshot snapshot() {
        return last == null ? null : new Snapshot(last.level * 100 / last.scale, last.status);
    }

    @Override
    public void scheduleUpdate() {
        poller.execute(this::update);
    }

    synchronized void update() {
        BatteryStateProvider.Reading reading = provider.read();
        if (reading == null) return;
        State state = new State(reading);
        if (state.sameAs(last)) return;
        State previous = last;
        last = state;
        broadcasts.broadcastAsSystem(batteryChanged(state, ++sequence), true);
        if (previous != null && (previous.plugged != 0) != (state.plugged != 0)) {
            broadcasts.broadcastAsSystem(new Intent(state.plugged != 0
                    ? Intent.ACTION_POWER_CONNECTED : Intent.ACTION_POWER_DISCONNECTED)
                    .addFlags(FLAG_RECEIVER_INCLUDE_BACKGROUND), false);
        }
        if (state.low() && !lowWarned) {
            lowWarned = true;
            broadcasts.broadcastAsSystem(new Intent(Intent.ACTION_BATTERY_LOW)
                    .addFlags(FLAG_RECEIVER_INCLUDE_BACKGROUND), false);
        } else if (lowWarned && (state.plugged != 0
                || state.level * 100 / state.scale >= LOW_BATTERY_CLOSE_WARNING_LEVEL)) {
            lowWarned = false;
            broadcasts.broadcastAsSystem(new Intent(Intent.ACTION_BATTERY_OKAY)
                    .addFlags(FLAG_RECEIVER_INCLUDE_BACKGROUND), false);
        }
    }

    // Intent.FLAG_RECEIVER_INCLUDE_BACKGROUND (hidden).
    private static final int FLAG_RECEIVER_INCLUDE_BACKGROUND = 0x01000000;

    /** BatteryService.sendBatteryChangedIntentLocked extras. */
    static Intent batteryChanged(State state, int sequence) {
        Intent intent = new Intent(Intent.ACTION_BATTERY_CHANGED);
        intent.addFlags(Intent.FLAG_RECEIVER_REGISTERED_ONLY
                | Intent.FLAG_RECEIVER_REPLACE_PENDING);
        intent.putExtra("seq", sequence); // BatteryManager.EXTRA_SEQUENCE
        intent.putExtra(BatteryManager.EXTRA_STATUS, state.status);
        intent.putExtra(BatteryManager.EXTRA_HEALTH, state.health);
        intent.putExtra(BatteryManager.EXTRA_PRESENT, state.present);
        intent.putExtra(BatteryManager.EXTRA_LEVEL, state.level);
        intent.putExtra(BatteryManager.EXTRA_BATTERY_LOW, state.low());
        intent.putExtra(BatteryManager.EXTRA_SCALE, state.scale);
        intent.putExtra(BatteryManager.EXTRA_PLUGGED, state.plugged);
        intent.putExtra(BatteryManager.EXTRA_VOLTAGE, 0);
        intent.putExtra(BatteryManager.EXTRA_TEMPERATURE, 0);
        intent.putExtra(BatteryManager.EXTRA_TECHNOLOGY, ""); // not reported by IOPowerSources
        return intent;
    }
}
