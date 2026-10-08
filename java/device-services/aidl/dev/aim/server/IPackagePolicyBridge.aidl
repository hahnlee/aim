package dev.aim.server;

/** Original non-PMS policy owners called by native PackageManager. */
interface IPackagePolicyBridge {
    void reportAdminPermissionDenied();
    int getInstallLocation();
    boolean setInstallLocation(int callerPid, int callerUid, int location);
    boolean isInstallDisabled(String packageName, int packageUid, int userId);
    boolean isPackageDeviceAdmin(String packageName, boolean packageExists);
    boolean queryPackageStateProtected(String packageName, int userId);
    boolean queryPackageDataProtected(String packageName, int userId);
    boolean isShellDebuggingRestricted(int userId);
    boolean isStorageLow();
}
