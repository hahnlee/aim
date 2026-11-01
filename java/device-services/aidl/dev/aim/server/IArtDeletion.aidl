package dev.aim.server;

/** Original ART deletion only. Native system owner supplies its live process generation. */
interface IArtDeletion {
    void clearAppProfiles(long generation, String packageName);
    long deleteDexoptArtifacts(long generation, String packageName, int callingUid, int userId);
}
