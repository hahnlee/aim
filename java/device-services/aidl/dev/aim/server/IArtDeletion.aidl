package dev.aim.server;

/** Original ART owner calls. Native system owner supplies its live process generation. */
interface IArtDeletion {
    void clearAppProfiles(long generation, String packageName);
    long deleteDexoptArtifacts(long generation, String packageName, int callingUid, int userId);
    /** PackageManagerService.systemReady's final DexUseManagerLocal.systemReady. */
    void systemReady(long generation);
}
