package dev.darwinart.runtime.power;

/**
 * The android.hardware.health IHealth facts behind the batteryproperties and
 * batterystats services, as BatteryService last reported them.
 */
public interface BatteryHealth {
    /** One reading: capacity in percent and a BatteryManager.BATTERY_STATUS_* value. */
    final class Snapshot {
        public final int capacity;
        public final int status;

        public Snapshot(int capacity, int status) {
            this.capacity = capacity;
            this.status = status;
        }
    }

    /** The latest reading, or null while the host power source is unavailable. */
    Snapshot snapshot();

    /** IHealth.update: re-read the host power source. */
    void scheduleUpdate();
}
