package dev.aim.server;

/** Original clone-provider policy; only the system UID supplies caller identity. */
interface IProviderQueries {
    boolean isCloneRedirected(String authority, int callingUid, int userId);
}
