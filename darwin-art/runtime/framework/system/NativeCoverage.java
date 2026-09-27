package dev.darwinart.runtime.system;

/**
 * `darwin-artctl native-coverage`: the state of every native method of the
 * pinned Android 16 jars in this process (crates/darwin-art-runtime
 * native_coverage.rs).
 */
final class NativeCoverage {
    private NativeCoverage() {}

    /** Classes resolve through the system server class loader, uninitialized. */
    static String report() {
        return nativeReport(NativeCoverage.class.getClassLoader());
    }

    private static native String nativeReport(ClassLoader loader);
}
