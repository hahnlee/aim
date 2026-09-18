package com.android.server.pm.dex;

/**
 * Compile signatures consumed by DexUsageStore, pinned to Android 16.
 * Original PackageDexUsage/AbstractStatsBase in services.jar own all behavior.
 * This class must never enter the adapter JAR or product DEX.
 */
public final class PackageDexUsage {
    public native boolean record(String owner, String path, int userId, String isa,
            boolean primaryOrSplit, String loader, String context, boolean overwriteContext);
    public native void read();
    public native void maybeWriteAsync();
    public native void writeNow();
}
