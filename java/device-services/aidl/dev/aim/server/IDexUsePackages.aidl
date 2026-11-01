package dev.aim.server;
import dev.aim.server.IPackageScanSnapshot;
import dev.aim.server.IArtDeletion;

/** Read-only native captures for ART's PackageManagerLocal; system UID only. */
interface IDexUsePackages {
    IPackageScanSnapshot getSnapshot();
    String filterPackageName(long version, String name, int uid, int userId);
    boolean shouldFilter(long version, String name, int uid, int userId);
    boolean crossUserSuspensions();
    /** ART attachment uses this same package epoch, after application-thread attachment. */
    long getArtOwnerGeneration();
    void attachArtOwner(long generation, IArtDeletion owner);
    void checkArtOwner(long generation);
}
