package dalvik.system;

/** Compile-only hidden-platform signatures; core-libart owns the runtime class. */
public final class DexFile {
    private DexFile() {}

    public static final class OptimizationInfo {
        private OptimizationInfo() {}

        public String getStatus() { throw new AssertionError(); }
        public String getReason() { throw new AssertionError(); }

        // Keep the compile-only companion's public API aligned with the
        // pinned platform class.  The implementation is never packaged into
        // the runtime DEX; it only supplies javac signatures for src-art.
        public boolean isVerified() { throw new AssertionError(); }
        public boolean isOptimized() { throw new AssertionError(); }
        public boolean isFullyCompiled() { throw new AssertionError(); }
    }
}
