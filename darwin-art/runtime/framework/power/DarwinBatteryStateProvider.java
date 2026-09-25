package dev.darwinart.runtime.power;

/** macOS IOPowerSources seam; native registration supplies the host reading. */
public final class DarwinBatteryStateProvider implements BatteryStateProvider {
    @Override
    public Reading read() {
        int[] values = nativeBatteryState();
        if (values == null) return null;
        return new Reading(values[0] != 0, values[1], values[2], values[3] != 0,
                values[4] != 0, values[5] != 0);
    }

    // {present, level, scale, charging, charged, externalPower}
    private static native int[] nativeBatteryState();
}
