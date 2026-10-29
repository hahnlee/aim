package dev.aim.server;
import dev.aim.server.IPackageScanSnapshot;

/** Read-only native captures for ART's PackageManagerLocal; system UID only. */
interface IDexUsePackages {
    IPackageScanSnapshot getSnapshot();
    String filterPackageName(long version, String name, int uid, int userId);
    boolean shouldFilter(long version, String name, int uid, int userId);
    boolean crossUserSuspensions();
}
