package dev.aim.server;
/** Pinned feature configuration and original installer storage primitives. */
interface IPackageEnableBridge {
    boolean quarantineEnabled();
    void clearCodeCache(String packageName, in int[] users);
}
