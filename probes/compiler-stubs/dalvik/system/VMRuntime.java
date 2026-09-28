package dalvik.system;

import java.util.function.Consumer;

/**
 * Compile-only surface for ART run-tests built against the platform
 * bootclasspath. The runtime definition always comes from core-libart.dex.
 */
public final class VMRuntime {
    public static final int CODE_PATH_TYPE_PRIMARY_APK = 1;
    public static final int CODE_PATH_TYPE_SPLIT_APK = 2;
    public static final int CODE_PATH_TYPE_SECONDARY_DEX = 4;

    public static VMRuntime getRuntime() { throw new AssertionError(); }
    public static String getCurrentInstructionSet() { throw new AssertionError(); }
    public static DexFile.OptimizationInfo getBaseApkOptimizationInfo() {
        throw new AssertionError();
    }
    public Object newNonMovableArray(Class<?> componentType, int length) {
        throw new AssertionError();
    }
    public Object newUnpaddedArray(Class<?> componentType, int length) {
        throw new AssertionError();
    }
    public boolean is64Bit() { throw new AssertionError(); }
    public long getFinalizerTimeoutMs() { throw new AssertionError(); }
    public void notifyStartupCompleted() { throw new AssertionError(); }
    public void notifyNativeAllocation() { throw new AssertionError(); }
    public void registerNativeAllocation(long bytes) { throw new AssertionError(); }
    public void registerNativeAllocation(int bytes) { throw new AssertionError(); }
    public void registerNativeFree(long bytes) { throw new AssertionError(); }
    public void registerNativeFree(int bytes) { throw new AssertionError(); }
    public void setTargetSdkVersion(int version) { throw new AssertionError(); }
    public void updateProcessState(int state) { throw new AssertionError(); }
    public static void resetJitCounters() { throw new AssertionError(); }
    public static void setNonSdkApiUsageConsumer(Consumer<String> consumer) {
        throw new AssertionError();
    }
    public static void registerAppInfo(
            String packageName,
            String currentProfile,
            String referenceProfile,
            String[] codePaths,
            int codePathType) {
        throw new AssertionError();
    }
}
