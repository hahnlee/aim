package dev.aim.server;
interface IPackageResolutionPolicy {
    boolean profilePermission(int callingUid, int targetUser, String packageName);
    boolean isKnownIsolatedComputeApp(int uid);
}
