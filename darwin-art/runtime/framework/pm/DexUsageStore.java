package com.android.server.pm.dex;

/** System-owned access to original AOSP usage storage; caller authenticates and resolves owners. */
public final class DexUsageStore {
    private final PackageDexUsage usage = new PackageDexUsage();

    public DexUsageStore() { usage.read(); }

    public void record(String owner, String path, int userId, String isa,
            boolean primaryOrSplit, String loader, String context, boolean overwriteContext) {
        if (usage.record(owner, path, userId, isa, primaryOrSplit, loader, context, overwriteContext)) {
            usage.maybeWriteAsync();
        }
    }

    public void writeNow() { usage.writeNow(); }
}
