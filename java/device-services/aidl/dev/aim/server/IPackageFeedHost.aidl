package dev.aim.server;

/**
 * The native PackageManager's model, fed by the original through the
 * bridge (IPackageFeed): batches of records, each the state of one
 * package, disabled system package, parsed package, shared user or user,
 * read through PackageManagerLocal and the system APIs and encoded as
 * crates/aim-services/src/package/feed.rs reads them. Each call is
 * synchronous, so system_server sends no faster than the host takes.
 */
interface IPackageFeedHost {
    /** A batch starts; with `reset`, the host drops every record first. */
    void begin(boolean reset);

    /**
     * Record `key` of `kind` is `chunk`, or starts with it: a record of
     * `length` bytes comes in chunks, in order, each small enough for a
     * transaction.
     */
    void put(int kind, String key, int length, in byte[] chunk);

    /** Record `key` of `kind` is gone. */
    void remove(int kind, String key);

    /**
     * The batch ends: `digest` is the SHA-256 of a fresh snapshot's
     * records, as the host computes it of the records it holds; `token`
     * is the latest IPackageFeed.sync's before the snapshot was taken (0
     * for none).
     */
    void end(in byte[] digest, long token);
}
