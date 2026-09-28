package dalvik.system;

/** Compiler-only Android 16 hidden-API surface; the boot class owns execution. */
public final class ZygoteHooks {
    private ZygoteHooks() {}

    public static void preFork() {}

    public static void postForkChild(
            int runtimeFlags,
            boolean isSystemServer,
            boolean isChildZygote,
            String instructionSet) {}

    public static void postForkCommon() {}
}
