// Compile-only pinned image API; never packaged.
package android.util;
public final class StatsEvent {
    private StatsEvent(int atomId, Buffer buffer, byte[] payload, int numBytes) {}
    private static final class Buffer { private Buffer() {} }
    public static Builder newBuilder() { throw new RuntimeException("stub"); }
    public static final class Builder {
        private Builder(Buffer buffer) {}
        public Builder setAtomId(int atom) { throw new RuntimeException("stub"); }
        public Builder writeInt(int value) { throw new RuntimeException("stub"); }
        public StatsEvent build() { throw new RuntimeException("stub"); }
    }
}
