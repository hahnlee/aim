package dev.aim.server;

/** Frozen native counters, flattened by the retained original health stats writer. */
interface IBatteryHealthStats {
    const int SINGLE = 0;
    const int ARRAY = 1;
    const int RESULT = 2;
    byte[] writeSnapshot(in long[] counters, int format);
}
