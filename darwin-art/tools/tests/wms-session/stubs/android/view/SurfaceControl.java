package android.view;

public class SurfaceControl {
    private boolean valid = true;
    public boolean isValid() { return valid; }
    public void release() { valid=false; }
    public void copyFrom(SurfaceControl other, String callsite) { valid=other != null && other.valid; }

    public static final class Builder {
        public Builder setName(String name) { return this; }
        public Builder setBufferSize(int width, int height) { return this; }
        public Builder setFormat(int format) { return this; }
        public SurfaceControl build() { return new SurfaceControl(); }
    }
    public static final class Transaction implements AutoCloseable {
        public Transaction setPosition(SurfaceControl surface, float x, float y) { return this; }
        public Transaction setLayer(SurfaceControl surface, int layer) { return this; }
        public Transaction reparent(SurfaceControl surface, SurfaceControl parent) { return this; }
        public void apply() {}
        @Override public void close() {}
    }
}
