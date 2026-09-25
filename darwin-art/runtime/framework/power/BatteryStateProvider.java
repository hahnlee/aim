package dev.darwinart.runtime.power;

/** Host power-source reading consumed by {@link BatteryService}. */
public interface BatteryStateProvider {
    /** A host power-source snapshot, as the Android health HAL reports it. */
    final class Reading {
        public final boolean present;
        public final int level;
        public final int scale;
        public final boolean charging;
        public final boolean charged;
        public final boolean externalPower;

        public Reading(boolean present, int level, int scale, boolean charging, boolean charged,
                boolean externalPower) {
            this.present = present;
            this.level = level;
            this.scale = scale;
            this.charging = charging;
            this.charged = charged;
            this.externalPower = externalPower;
        }
    }

    /** Returns the current reading, or null when the host source is unavailable. */
    Reading read();
}
