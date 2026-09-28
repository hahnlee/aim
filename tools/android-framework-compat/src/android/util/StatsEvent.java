package android.util;

/**
 * Process-local stats event used when Android runs without statsd.
 *
 * <p>The platform-generated FrameworkStatsLog code still builds events for
 * interaction telemetry. A detached application runtime has no statsd socket,
 * but retaining the builder contract keeps framework behavior independent of
 * that optional system service.</p>
 */
public final class StatsEvent {
    private StatsEvent() {}

    public static Builder newBuilder() {
        return new Builder();
    }

    public static final class Builder {
        public Builder setAtomId(int atomId) { return this; }
        public Builder writeInt(int value) { return this; }
        public Builder writeLong(long value) { return this; }
        public Builder writeFloat(float value) { return this; }
        public Builder writeBoolean(boolean value) { return this; }
        public Builder writeString(String value) { return this; }
        public Builder writeByteArray(byte[] value) { return this; }
        public Builder writeIntArray(int[] value) { return this; }
        public Builder writeLongArray(long[] value) { return this; }
        public Builder writeFloatArray(float[] value) { return this; }
        public Builder writeBooleanArray(boolean[] value) { return this; }
        public Builder writeStringArray(String[] value) { return this; }
        public Builder writeAttributionChain(int[] uids, String[] tags) { return this; }
        public Builder addBooleanAnnotation(byte annotationId, boolean value) { return this; }
        public Builder addIntAnnotation(byte annotationId, int value) { return this; }
        public Builder usePooledBuffer() { return this; }
        public StatsEvent build() { return new StatsEvent(); }
    }
}
