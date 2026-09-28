package dev.aim.managedload;

public final class ManagedNativeLoad {
    private ManagedNativeLoad() {}

    private static native int nativeProbe();

    public static int loadAbsolute(String absolutePath) {
        System.load(absolutePath);
        return nativeProbe();
    }
}
