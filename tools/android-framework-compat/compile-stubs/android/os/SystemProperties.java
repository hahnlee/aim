package android.os;

/** Compile-only declaration; Android's framework DEX owns the runtime class. */
public final class SystemProperties {
    private SystemProperties() {}

    public static int getInt(String key, int defaultValue) {
        throw new AssertionError("compile-only stub");
    }
}
