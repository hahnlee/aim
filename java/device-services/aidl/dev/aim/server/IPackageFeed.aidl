package dev.aim.server;

/**
 * The feed of the original PackageManager's state to the native
 * PackageManager's model (docs/m4-packagemanager.md, slice A), the
 * system_server side: IBridge.getPackageFeed hands it to the host.
 */
oneway interface IPackageFeed {
    /**
     * Sends the host what changed since the last batch, or, with `reset`,
     * everything, then the digest of a fresh snapshot
     * (IPackageFeedHost.end). The batch taken after this call ends with
     * `token` or a later one.
     */
    void sync(boolean reset, long token);
}
