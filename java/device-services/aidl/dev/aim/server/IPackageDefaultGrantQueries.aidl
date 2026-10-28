package dev.aim.server;
/** Native package owners used by the original DefaultPermissionGrantPolicy. System UID only. */
interface IPackageDefaultGrantQueries {
    String[] getKnownPackageNames(int kind, int userId);
    String getDisabledSystemPackageName(String packageName);
    boolean isPlatformSigned(String packageName);
    boolean isResolveActivityComponent(String packageName, String className);
}
